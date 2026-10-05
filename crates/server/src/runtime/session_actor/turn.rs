use std::sync::Arc;

use tokio::sync::mpsc;

use crate::runtime::ServerRuntime;
use crate::runtime::session_actor::TurnWorkingSet;
use crate::runtime::subagent_usage::UsageTotals;
use crate::runtime::turn_exec::{
    ExecuteTurnRequest, FinalizeTurnParams, QUERY_EVENT_CHANNEL_CAPACITY, TurnModelQueryParams,
    spawn_turn_event_stream,
};

/// Runs one turn on the caller's task using a checked-out [`TurnWorkingSet`].
///
/// Returns whether goal continuation should be considered after merge.
/// Post-turn scheduling is the caller's responsibility so this future does not
/// recursively type-check against queue/follow-up spawn paths.
pub(crate) async fn execute_turn_task(
    mut working: TurnWorkingSet,
    runtime: Arc<ServerRuntime>,
    request: ExecuteTurnRequest,
) -> bool {
    let ExecuteTurnRequest {
        session_id,
        turn,
        turn_config,
        display_input,
        client_user_message_id,
        input,
        input_messages,
        input_images,
        input_image_paths,
        collaboration_mode,
        input_mode,
        user_message_already_emitted,
    } = request;

    let pre_query_started = std::time::Instant::now();
    let spawn_snapshot = Arc::new(working.state.spawn_snapshot());
    let turn_id = turn.turn_id();
    let mut step_started = std::time::Instant::now();
    runtime
        .register_turn_spawn_snapshot(session_id, turn_id, Arc::clone(&spawn_snapshot))
        .await;
    let spawn_register_ms = step_started.elapsed().as_millis() as u64;

    step_started = std::time::Instant::now();
    runtime
        .register_active_stream(session_id, Arc::clone(&working.state.stream))
        .await;
    let stream_register_ms = step_started.elapsed().as_millis() as u64;

    step_started = std::time::Instant::now();
    runtime
        .prepare_turn_execution_for_actor(
            &mut working.state,
            &turn,
            &display_input,
            &input_image_paths,
            input_mode.emits_user_message() && !user_message_already_emitted,
            client_user_message_id.as_deref(),
        )
        .await;
    let prepare_ms = step_started.elapsed().as_millis() as u64;

    let (event_tx, event_rx) = mpsc::channel(QUERY_EVENT_CHANNEL_CAPACITY);
    step_started = std::time::Instant::now();
    let event_tool_registry = runtime.tool_registry_for_actor_state(&working.state);
    let tool_registry_ms = step_started.elapsed().as_millis() as u64;
    let usage_parent_session_id = working.state.parent_session_id();
    let usage_context_window = Some(crate::runtime::context_occupancy::occupancy_window_tokens(
        Some(&turn_config.model),
    ));
    step_started = std::time::Instant::now();
    if usage_parent_session_id.is_none() {
        runtime
            .begin_parent_usage_turn_with_base(
                session_id,
                turn_id,
                UsageTotals::from_session_summary(&working.state.summary),
                usage_context_window,
            )
            .await;
    }
    let usage_ledger_ms = step_started.elapsed().as_millis() as u64;

    tracing::info!(
        session_id = %session_id,
        pre_query_setup_ms = pre_query_started.elapsed().as_millis() as u64,
        spawn_register_ms,
        stream_register_ms,
        prepare_ms,
        tool_registry_ms,
        usage_ledger_ms,
        "turn task: pre-query setup completed (baseline, usage ledger, event stream)"
    );
    let stream = Arc::clone(&working.state.stream);
    let event_task = spawn_turn_event_stream(
        Arc::clone(&runtime),
        stream,
        session_id,
        turn.clone(),
        collaboration_mode,
        event_tool_registry,
        usage_parent_session_id,
        usage_context_window,
        event_rx,
    );

    let query_outcome = runtime
        .run_turn_model_query(TurnModelQueryParams {
            state: &mut working.state,
            turn_id,
            turn_config: &turn_config,
            input: &input,
            input_messages: &input_messages,
            input_images: &input_images,
            collaboration_mode,
            input_mode,
            usage_parent_session_id: usage_parent_session_id.as_ref().map(|id| *id),
            event_tx,
        })
        .await;
    let event_summary = event_task.await.ok();

    runtime
        .finalize_executed_turn(FinalizeTurnParams {
            state: &mut working.state,
            session_id,
            turn,
            query_outcome,
            event_summary,
            usage_parent_session_id: usage_parent_session_id.as_ref().map(|id| *id),
        })
        .await;

    // Merge before clearing the runtime registry so admission (compact /
    // turn/start) cannot see a free registry while the actor still holds
    // `active_turn` from BeginActiveTurn.
    let inline = {
        let mut stream = working.state.stream.lock().await;
        stream.turn_inline.take()
    };
    if let Some(inline) = inline {
        inline.merge_into(&mut working.state);
    }

    let should_auto_continue_goal = working.state.latest_turn.as_ref().is_some_and(|turn| {
        matches!(
            turn.native.status,
            devo_protocol::native::turn::TurnStatus::Completed
                | devo_protocol::native::turn::TurnStatus::Failed
        )
    });

    if let Some(handle) = runtime.session(session_id).await {
        handle.merge_turn(working).await;
    }

    runtime.clear_turn_spawn_snapshot(session_id, turn_id).await;
    runtime.unregister_active_stream(session_id).await;
    runtime
        .clear_active_turn_interrupt_handles(session_id)
        .await;
    runtime.clear_active_turn_runtime_handles(session_id).await;
    runtime.broadcast_recovery_state(session_id).await;

    should_auto_continue_goal
}
