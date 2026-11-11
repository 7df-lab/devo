//! Restricted idle host callback for detached Bash completion notices and withdrawals.

use std::sync::{Arc, Weak};

use futures::future::BoxFuture;
use serde_json::{Value, json};

use devo_protocol::native::ids::SessionId;

use super::ServerRuntime;

/// Handles notices and session-scoped handle controls; idle process starts are denied.
pub(crate) fn for_session(
    runtime: Weak<ServerRuntime>,
    session_id: SessionId,
) -> devo_kernel::HostRequestHandler {
    let session_key = session_id.to_string();
    Arc::new(move |_request_id, data| {
        let runtime = runtime.clone();
        let session_id = session_id;
        let session_key = session_key.clone();
        Box::pin(async move {
            let (action, params) = devo_kernel::parse_host_request_data(&data);
            match action.as_str() {
                "bash.completed" => {
                    super::kernel_host::push_bash_notice(
                        &session_key,
                        json!({
                            "kind": "async_bash_completion",
                            "params": params,
                        }),
                    );
                    if let Some(runtime) = runtime.upgrade() {
                        runtime
                            .drain_async_tool_completion_notices(session_id)
                            .await;
                    }
                }
                "bash.consumed" => {
                    let Some(identity) = params
                        .get("handle_id")
                        .filter(|value| value.is_string())
                        .or_else(|| params.get("pid").filter(|value| value.is_i64()))
                    else {
                        return json!({
                            "status": "error",
                            "error": "bash.consumed requires a handle_id or pid",
                        });
                    };
                    super::kernel_host::consume_bash_notice(&session_key, identity);
                }
                "bash.poll" | "bash.kill" | "bash.forget" => {
                    return devo_core::tools::host_bash::control(&session_key, &action, &params);
                }
                _ => {
                    return json!({
                        "status": "error",
                        "error": format!("idle host request denied: {action}"),
                    });
                }
            }
            json!({"status": "ok", "result": {"accepted": true}})
        }) as BoxFuture<'static, Value>
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;
    use crate::runtime::kernel_host::take_bash_notices;

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use anyhow::{Context, Result};
    use async_trait::async_trait;
    use devo_protocol::{ModelRequest, ModelResponse, StreamEvent};
    use devo_provider::ModelProviderSDK;
    use tempfile::TempDir;
    use uuid::Uuid;

    use crate::ClientTransportKind;
    use crate::SuccessResponse;
    use crate::runtime::outbound::test_outbound_channel;
    use crate::test_support::TestRuntime;

    struct GatedProvider {
        open: Arc<AtomicBool>,
        started: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ModelProviderSDK for GatedProvider {
        fn name(&self) -> &str {
            "gated"
        }

        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            anyhow::bail!("gated provider does not support completion")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>>
        {
            self.started.store(true, Ordering::SeqCst);
            let open = Arc::clone(&self.open);
            Ok(Box::pin(futures::stream::unfold(false, move |done| {
                let open = Arc::clone(&open);
                async move {
                    if done {
                        return None;
                    }
                    while !open.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Some((
                        Ok(StreamEvent::MessageDone {
                            response: ModelResponse {
                                id: "idle-bash-followup".into(),
                                content: vec![devo_protocol::ResponseContent::Text("done".into())],
                                stop_reason: Some(devo_protocol::StopReason::EndTurn),
                                usage: devo_protocol::Usage::default(),
                                metadata: devo_protocol::ResponseMetadata::default(),
                            },
                        }),
                        true,
                    ))
                }
            })))
        }
    }

    #[tokio::test]
    async fn idle_host_handler_only_enqueues_bash_completion() {
        let session_id = SessionId::new();
        let session_key = session_id.to_string();
        let handler = for_session(Weak::new(), session_id);
        let reply = handler(
            "completion-1".into(),
            json!({
                "type": "bash.completed",
                "params": { "pid": 123, "exitCode": 0 },
            }),
        )
        .await;
        assert_eq!(reply["status"], "ok");
        assert_eq!(reply["result"]["accepted"], true);

        let notice = json!({
            "kind": "async_bash_completion",
            "params": { "pid": 123, "exitCode": 0 },
        });
        assert_eq!(take_bash_notices(&session_key), vec![notice.clone()]);
        super::super::kernel_host::push_bash_notice(&session_key, notice);
        let consumed = handler(
            "consumed-1".into(),
            json!({ "type": "bash.consumed", "params": { "pid": 123 } }),
        )
        .await;
        assert_eq!(consumed["status"], "ok");
        assert!(take_bash_notices(&session_key).is_empty());

        let denied = handler(
            "unsafe-1".into(),
            json!({ "type": "fs.write", "params": {} }),
        )
        .await;
        assert_eq!(denied["status"], "error");
        assert!(take_bash_notices(&session_key).is_empty());
    }

    #[tokio::test]
    async fn idle_controls_are_scoped_and_start_stays_denied() {
        let session_id = SessionId::new();
        let handler = for_session(Weak::new(), session_id);
        for action in ["bash.poll", "bash.kill", "bash.forget"] {
            assert_eq!(
                handler(
                    "control".into(),
                    json!({"type":action, "params":{"handle_id":"missing"}})
                )
                .await,
                json!({"status":"error", "error":"unknown bash handle"})
            );
        }
        assert_eq!(
            handler(
                "start".into(),
                json!({"type":"bash.start", "params":{"command":"echo nope"}})
            )
            .await,
            json!({"status":"error", "error":"idle host request denied: bash.start"})
        );
        handler(
            "done".into(),
            json!({"type":"bash.completed", "params":{"handle_id":"opaque"}}),
        )
        .await;
        handler(
            "consumed".into(),
            json!({"type":"bash.consumed", "params":{"handle_id":"opaque"}}),
        )
        .await;
        assert_eq!(take_bash_notices(session_id.as_str()), Vec::<Value>::new());
    }

    #[tokio::test]
    async fn completion_notice_is_restored_when_follow_up_turn_cannot_start() {
        let data_root = TempDir::new().expect("tempdir");
        let runtime = TestRuntime::new(Arc::new(GatedProvider {
            open: Arc::new(AtomicBool::new(false)),
            started: Arc::new(AtomicBool::new(false)),
        }))
        .db_file("failed-idle-bash.db")
        .runtime(data_root.path());
        let session_id = SessionId::new();
        let session_key = session_id.to_string();
        let notice = json!({
            "kind": "async_bash_completion",
            "params": { "pid": 902, "exitCode": 1 },
        });
        crate::runtime::kernel_host::push_bash_notice(&session_key, notice.clone());

        runtime
            .drain_async_tool_completion_notices(session_id)
            .await;

        assert_eq!(take_bash_notices(&session_key), vec![notice]);
    }

    #[tokio::test]
    async fn idle_bash_completion_starts_a_follow_up_turn() -> Result<()> {
        idle_completion_after_goal_transition(/*goal_status*/ None).await
    }

    #[tokio::test]
    async fn terminal_goals_only_suppress_their_own_late_completion_notices() -> Result<()> {
        for status in [
            crate::goal::GoalStatus::Completed,
            crate::goal::GoalStatus::Canceled,
            crate::goal::GoalStatus::Cleared,
        ] {
            idle_completion_after_goal_transition(Some(status)).await?;
        }
        Ok(())
    }

    async fn idle_completion_after_goal_transition(
        goal_status: Option<crate::goal::GoalStatus>,
    ) -> Result<()> {
        let open = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let data_root = TempDir::new()?;
        let runtime = TestRuntime::new(Arc::new(GatedProvider {
            open: Arc::clone(&open),
            started: Arc::clone(&started),
        }))
        .db_file("idle-bash.db")
        .runtime(data_root.path());
        let (outbound, _receiver) = test_outbound_channel(8);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        runtime
            .handle_acp_initialize(
                connection_id,
                Some(json!(1)),
                json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        let response = runtime
            .handle_incoming(
                connection_id,
                json!({
                    "id": 2,
                    "method": "session/new",
                    "params": {
                        "cwd": data_root.path(),
                        "idempotencyKey": format!("idle-bash-session-{}", Uuid::new_v4()),
                    },
                }),
            )
            .await
            .context("session/new response")?;
        let session: SuccessResponse<devo_protocol::native::rpc_session::SessionNewResult> =
            serde_json::from_value(response)?;
        let session_id = session.result.session.id;
        let session_key = session_id.to_string();

        let handler = for_session(Arc::downgrade(&runtime), session_id);
        if let Some(status) = goal_status {
            let mut store = crate::runtime::GoalStore::new();
            let goal = store.create(devo_protocol::GoalCreateParams {
                session_id,
                objective: "old goal".to_string(),
                token_budget: None,
                replace_existing: false,
            })?;
            runtime
                .sync_core_session_goal(session_id, Some(&goal))
                .await;
            runtime.register_background_work(session_id, "bash:old-goal-handle".to_string());
            if status == crate::goal::GoalStatus::Cleared {
                store.clear();
            } else {
                store.set_status(status)?;
            }
            runtime.goal_stores.lock().await.insert(session_id, store);
            runtime.cancel_background_goal_wait(session_id);
            runtime.sync_core_session_goal(session_id, None).await;
            runtime.complete_background_work(
                session_id,
                "bash:old-goal-handle".to_string(),
                "old result".to_string(),
            );
            handler(
                "late-old-completion".into(),
                json!({
                    "type": "bash.completed",
                    "params": {"handle_id": "old-goal-handle", "exitCode": 0}
                }),
            )
            .await;
            assert!(
                !started.load(Ordering::SeqCst),
                "old goal must not wake from a late notice"
            );
            assert!(take_bash_notices(&session_key).is_empty());
        }
        runtime.register_background_work(session_id, "bash:new-chat-handle".to_string());
        devo_core::tools::AgentToolCoordinator::background_work_completed(
            runtime.as_ref(),
            session_id,
            "bash:new-chat-handle".to_string(),
            "new ordinary chat result".to_string(),
        );
        let reply = handler(
            "bash-completion-1".into(),
            json!({
                "type": "bash.completed",
                "params": { "handle_id": "new-chat-handle", "command": "echo test", "exitCode": 0 },
            }),
        )
        .await;
        assert_eq!(reply["status"], "ok");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("completion notice did not wake the idle session")?;
        let reservation = runtime
            .session_turn_reservation_snapshot(session_id)
            .await
            .context("follow-up turn reservation")?;
        let turn_id = reservation
            .active_turn
            .as_ref()
            .expect("idle completion starts a turn")
            .turn_id();
        assert!(
            runtime
                .active_turn_user_texts(session_id, turn_id)
                .await
                .iter()
                .any(|text| text.contains("new ordinary chat result"))
        );
        assert!(take_bash_notices(&session_key).is_empty());
        open.store(true, Ordering::SeqCst);
        Ok(())
    }
}
