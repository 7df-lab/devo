use super::*;
use pretty_assertions::assert_eq;

/// Verifies: child sessions cannot schedule refine.run.
#[test]
fn children_cannot_schedule() {
    let id = devo_protocol::native::ids::SessionId::from_string("ses_child".into());
    let params = SessionRefineRunParams {
        session_id: id,
        instructions: None,
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    let result = schedule_refine_run(&id, &params, false, None);
    assert!(!result.scheduled);
}

/// Verifies: root schedule last-write-wins pending refine.
#[test]
fn root_schedule_is_pending() {
    let id = devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
    let params = SessionRefineRunParams {
        session_id: id,
        instructions: Some("focus".into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    let result = schedule_refine_run(&id, &params, true, None);
    assert!(result.scheduled);
    assert!(peek_pending_refine(&id));
    let taken = take_pending_refine(&id).expect("pending");
    assert_eq!(taken.instructions.as_deref(), Some("focus"));
    assert!(!taken.autonomous);
}

/// Verifies: should_auto_refine integration schedules on interval for roots.
#[test]
fn auto_interval_schedules_on_root() {
    let id = devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
    let settings = AutoRefineSettings {
        enabled: true,
        turn_interval: 2,
        ..AutoRefineSettings::default()
    };
    assert!(!maybe_schedule_auto_refine(
        &id, &settings, true, false, true, None
    ));
    assert!(!peek_pending_refine(&id));
    assert!(maybe_schedule_auto_refine(
        &id, &settings, true, false, true, None
    ));
    assert!(peek_pending_refine(&id));
    let taken = take_pending_refine(&id).expect("auto pending");
    assert!(taken.autonomous);
    assert_eq!(taken.instructions.as_deref(), Some(AUTO_REFINE_INSTRUCTION));
}

/// Verifies: children never auto-refine even at interval.
#[test]
fn auto_interval_skips_children() {
    let id = devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
    let settings = AutoRefineSettings {
        turn_interval: 1,
        ..AutoRefineSettings::default()
    };
    assert!(!maybe_schedule_auto_refine(
        &id, &settings, false, false, true, None
    ));
    assert!(!peek_pending_refine(&id));
}

/// Verifies: boundary apply writes refinements.jsonl (never mid-ipython).
#[test]
fn apply_at_boundary_persists_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let id = devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
    let params = SessionRefineRunParams {
        session_id: id,
        instructions: Some("manual".into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    assert!(schedule_refine_run(&id, &params, true, Some(dir.path())).scheduled);
    let applied = apply_pending_refine_at_boundary(&id, dir.path(), false).expect("applied");
    assert!(!applied.proposal.id.is_empty());
    let log = std::fs::read_to_string(devo_harness::refinements_log_path(dir.path())).unwrap();
    assert!(log.contains(&applied.proposal.id));
    assert!(!peek_pending_refine(&id));
}

/// Verifies: Plan Mode holds autonomous pending without applying.
#[test]
fn plan_mode_defers_autonomous_apply() {
    let dir = tempfile::tempdir().unwrap();
    let id = devo_protocol::native::ids::SessionId::from_string(format!("ses_{}", Uuid::new_v4()));
    let settings = AutoRefineSettings {
        turn_interval: 1,
        ..AutoRefineSettings::default()
    };
    assert!(maybe_schedule_auto_refine(
        &id,
        &settings,
        true,
        false,
        true,
        Some(dir.path())
    ));
    assert!(apply_pending_refine_at_boundary(&id, dir.path(), true).is_none());
    assert!(peek_pending_refine(&id));
    clear_pending_refine(&id);
}

#[test]
fn default_interval_matches_spec() {
    assert_eq!(devo_harness::DEFAULT_TURN_INTERVAL, 25);
    assert_eq!(
        auto_refine_settings_from_session(None, None).turn_interval,
        25
    );
    assert!(auto_refine_settings_from_session(None, None).enabled);
    assert!(!auto_refine_settings_from_session(Some(false), Some(10)).enabled);
    assert_eq!(
        auto_refine_settings_from_session(Some(true), Some(10)).turn_interval,
        10
    );
}

/// Verifies: plan_refine_proposal with instructions yields a memory Create edit.
#[test]
fn plan_refine_proposal_with_instructions_emits_memory_create() {
    let dir = tempfile::tempdir().unwrap();
    let path = HarnessState::file_path(dir.path());
    HarnessState::default().save_atomic(&path).unwrap();
    let pending = PendingRefine {
        proposal_id: "refine_test".into(),
        instructions: Some("  focus on harness digest wiring  ".into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
        trajectory_evidence: None,
        requested_at: Utc::now(),
        autonomous: false,
        prepared: None,
    };
    let proposal = plan_refine_proposal(&pending, &path);
    assert_eq!(proposal.id, "refine_test");
    assert!(!proposal.edits.is_empty());
    assert_eq!(proposal.edits.len(), 1);
    let edit = &proposal.edits[0];
    assert_eq!(edit.op, RefineEditOp::Create);
    assert_eq!(edit.kind, HarnessKind::Memory);
    assert!(edit.id.starts_with("mem_refine_"));
    let after = edit.after.as_ref().expect("after");
    assert!(after.content.contains("harness digest"));
    assert!(proposal.summary.contains("harness digest"));
    assert!(proposal.evidence.contains("harness digest"));
    assert!(proposal.expected_outcome.contains(&edit.id));
}

/// Verifies: empty/missing instructions keep the no-op empty-edits proposal.
#[test]
fn plan_refine_proposal_without_instructions_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = HarnessState::file_path(dir.path());
    HarnessState::default().save_atomic(&path).unwrap();
    let pending = PendingRefine {
        proposal_id: "refine_empty".into(),
        instructions: Some("   ".into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
        trajectory_evidence: None,
        requested_at: Utc::now(),
        autonomous: false,
        prepared: None,
    };
    let proposal = plan_refine_proposal(&pending, &path);
    assert!(proposal.edits.is_empty());
    let pending_none = PendingRefine {
        instructions: None,
        ..pending
    };
    assert!(plan_refine_proposal(&pending_none, &path).edits.is_empty());
}

/// Verifies: the auto-interval sentinel is a scheduling marker, not
/// guidance — the heuristic must not echo it into a durable memory
/// (bug #48), while the durable trigger keeps the sentinel label.
#[test]
fn plan_refine_proposal_auto_sentinel_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = HarnessState::file_path(dir.path());
    HarnessState::default().save_atomic(&path).unwrap();
    let pending = PendingRefine {
        proposal_id: "refine_auto".into(),
        instructions: Some(AUTO_REFINE_INSTRUCTION.into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
        trajectory_evidence: None,
        requested_at: Utc::now(),
        autonomous: true,
        prepared: None,
    };
    let proposal = plan_refine_proposal(&pending, &path);
    assert!(
        proposal.edits.is_empty(),
        "sentinel echoed as edits: {:?}",
        proposal.edits
    );
    assert_eq!(proposal.trigger, AUTO_REFINE_INSTRUCTION);
}

#[test]
fn turns_since_last_refinement_counts_regular_turns_after_last_refine() {
    use crate::persisted_native_item::PersistedNativeItem;
    use devo_protocol::native::ids::{ItemId, TurnId};
    use devo_protocol::native::item::Item;
    use devo_protocol::native::turn::TurnKind;

    let item = |turn: TurnId, kind: TurnKind, id: &str| {
        PersistedNativeItem::new(
            turn,
            kind,
            ItemId::from_string(id.to_string()),
            Item::AssistantMessage {
                text: format!("item {id}"),
            },
        )
    };
    let t1 = TurnId::new();
    let t2 = TurnId::new();
    let t3 = TurnId::new();
    let refine_turn = TurnId::new();
    let journal = vec![
        item(t1, TurnKind::Regular, "a1"),
        item(t1, TurnKind::Regular, "a2"),
        // Compaction turns are bookkeeping, not user-visible turns.
        item(TurnId::new(), TurnKind::Compaction, "c1"),
        item(t2, TurnKind::Regular, "b1"),
        // A refinement applied at t2's boundary.
        PersistedNativeItem::new(
            refine_turn,
            TurnKind::Regular,
            ItemId::from_string("r1".to_string()),
            Item::Refinement {
                refinement_id: "refine_1".to_string(),
                trigger: "auto-interval".to_string(),
                summary: "applied".to_string(),
                changes: Vec::new(),
                evidence: None,
                outcome: None,
            },
        ),
        item(t3, TurnKind::Regular, "d1"),
        item(t3, TurnKind::Regular, "d2"),
    ];

    // Only t3 (after the refinement marker) counts.
    assert_eq!(turns_since_last_refinement(&journal), 1);
    // Without any refinement marker, every regular turn counts
    // (restart must not silently reset the interval to zero).
    assert_eq!(turns_since_last_refinement(&journal[..4]), 2);
}

/// Verifies: heuristic planner entries and records follow the requested
/// scope, and the Devo home resolves to the global harness layout.
#[test]
fn global_refine_targets_global_store() {
    let global_pending = PendingRefine {
        proposal_id: "refine_global_test".into(),
        instructions: Some("remember the west wing passphrase".into()),
        global: true,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
        trajectory_evidence: None,
        requested_at: Utc::now(),
        autonomous: false,
        prepared: None,
    };
    assert_eq!(refine_target_scope(&global_pending), HarnessScope::Global);
    assert_eq!(refine_scope_label(&global_pending), "global");
    let proposal = plan_refine_proposal(&global_pending, Path::new("/unused"));
    let entry = proposal.edits[0].after.as_ref().expect("heuristic entry");
    assert_eq!(entry.scope, Some(HarnessScope::Global));

    let local_pending = PendingRefine {
        proposal_id: "refine_local_test".into(),
        instructions: Some("remember the session color".into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
        trajectory_evidence: None,
        requested_at: Utc::now(),
        autonomous: false,
        prepared: None,
    };
    assert_eq!(refine_target_scope(&local_pending), HarnessScope::Local);
    let session_dir = tempfile::tempdir().expect("session dir");
    assert_eq!(
        refine_target_harness_path(session_dir.path(), &local_pending).expect("local path"),
        session_dir
            .path()
            .join("harness")
            .join("harness_state.json")
    );
    assert_eq!(refine_scope_label(&local_pending), "local");
    let proposal = plan_refine_proposal(&local_pending, Path::new("/unused"));
    let entry = proposal.edits[0].after.as_ref().expect("heuristic entry");
    assert_eq!(entry.scope, Some(HarnessScope::Local));

    // Passing the Devo home as the "session dir" of the store helper is
    // exactly how refine_target_harness_path resolves the global store:
    // <home>/harness/harness_state.json.
    let home = tempfile::tempdir().expect("home dir");
    let path = ensure_session_harness(home.path()).expect("global store path");
    assert_eq!(path, home.path().join("harness").join("harness_state.json"));
    assert!(path.is_file());
}

#[test]
fn scheduling_does_not_touch_harness_storage() {
    let directory = tempfile::tempdir().unwrap();
    let session_id = devo_protocol::native::ids::SessionId::new();
    let params = SessionRefineRunParams {
        session_id,
        instructions: Some("remember a lesson".into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    let result = schedule_refine_run(
        &session_id,
        &params,
        /*is_root*/ true,
        Some(directory.path()),
    );
    assert!(result.scheduled);
    assert!(!directory.path().join("harness").exists());
    assert!(take_pending_refine(&session_id).is_some());
}
#[test]
fn rollback_restores_entries_and_missing_history_reports_failure() {
    let dir = tempfile::tempdir().unwrap();
    let id = devo_protocol::native::ids::SessionId::new();
    let params = SessionRefineRunParams {
        session_id: id,
        instructions: Some("Run the narrow regression first".into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    assert!(schedule_refine_run(&id, &params, true, Some(dir.path())).scheduled);
    let applied = apply_pending_refine_at_boundary(&id, dir.path(), false).unwrap();
    assert!(applied.failure.is_none());
    let params = SessionRefineRunParams {
        instructions: None,
        rollback_id: Some(applied.proposal.id),
        ..params
    };
    assert!(schedule_refine_run(&id, &params, true, Some(dir.path())).scheduled);
    let undone = apply_pending_refine_at_boundary(&id, dir.path(), false).unwrap();
    assert!(undone.failure.is_none());
    let state = HarnessState::load(&HarnessState::file_path(dir.path())).unwrap();
    assert!(state.entries["memory"].is_empty());
    assert_eq!(state.refinement_history.len(), 2);
    assert!(schedule_refine_run(&id, &params, true, Some(dir.path())).scheduled);
    let duplicate = apply_pending_refine_at_boundary(&id, dir.path(), false).unwrap();
    assert!(
        matches!(duplicate.native_item(), Item::Warning { code, .. } if code == "refinement_not_applied")
    );
    assert_eq!(
        HarnessState::load(&HarnessState::file_path(dir.path())).unwrap(),
        state
    );
}

#[test]
fn manual_plan_mode_defers_and_mirror_failure_keeps_undo() {
    let dir = tempfile::tempdir().unwrap();
    let id = devo_protocol::native::ids::SessionId::new();
    let params = SessionRefineRunParams {
        session_id: id,
        instructions: Some("Remember testing".into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    assert!(schedule_refine_run(&id, &params, true, Some(dir.path())).scheduled);
    assert!(apply_pending_refine_at_boundary(&id, dir.path(), true).is_none());
    assert!(!dir.path().join("harness").exists());
    std::fs::create_dir_all(devo_harness::refinements_log_path(dir.path())).unwrap();
    let applied = apply_pending_refine_at_boundary(&id, dir.path(), false).unwrap();
    assert!(applied.failure.is_none());
    assert_eq!(
        HarnessState::load(&HarnessState::file_path(dir.path()))
            .unwrap()
            .refinement_history[0]
            .id,
        applied.proposal.id
    );
}

#[test]
fn trajectory_cannot_be_global_autonomous_or_combined_with_rollback() {
    let id = devo_protocol::native::ids::SessionId::new();
    let mut params = SessionRefineRunParams {
        session_id: id,
        instructions: None,
        global: true,
        rollback_id: None,
        trajectory_item_ids: vec![devo_protocol::native::ids::ItemId::new()],
    };
    assert!(!schedule_refine_run(&id, &params, true, None).scheduled);
    params.global = false;
    params.rollback_id = Some("old".into());
    assert!(!schedule_refine_run(&id, &params, true, None).scheduled);
    params.rollback_id = None;
    assert!(!schedule_refine_run_inner(&id, &params, true, None, true, Some(json!([]))).scheduled);
    // Kernel-facing scheduling cannot supply user-selected evidence.
    assert!(!schedule_refine_run(&id, &params, true, None).scheduled);
    assert!(schedule_refine_run_inner(&id, &params, true, None, false, Some(json!([]))).scheduled);
    let pending = take_pending_refine(&id).unwrap();
    assert!(
        plan_refine_proposal(&pending, Path::new("unused"))
            .edits
            .is_empty()
    );
}

#[tokio::test]
async fn idle_manual_refine_broadcasts_failure_instead_of_silent_noop() {
    let (_temp, runtime, connection, session_id, mut rx) = refine_test_session().await;
    let response = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 3, "method": "session/refine/run", "params": { "sessionId": session_id }
            }),
        )
        .await
        .unwrap();
    assert_eq!(response["result"]["scheduled"], json!(true));
    let notification = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let notification = rx.recv().await.expect("connection open");
            if notification["method"] == "item/completed"
                && notification["params"]["item"]["item"]["code"] == "refinement_not_applied"
            {
                break notification;
            }
        }
    })
    .await
    .expect("idle refine must publish outcome without a user turn");
    assert_eq!(
        notification["params"]["item"]["sessionId"],
        json!(session_id)
    );
    assert!(!peek_pending_refine(&session_id));
    let result_id = notification["params"]["item"]["id"].clone();
    let history = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 4, "method": "session/items/list", "params": { "sessionId": session_id }
            }),
        )
        .await
        .unwrap();
    assert!(
        history["result"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == result_id),
        "refine result must survive history projection: {history}"
    );
}

#[tokio::test]
async fn manual_refine_waits_for_busy_auxiliary_slot_instead_of_empty_success() {
    let temp = tempfile::TempDir::new().unwrap();
    let runtime = crate::test_support::TestRuntime::noop().runtime(temp.path());
    let session_id = SessionId::new();
    let params = SessionRefineRunParams {
        session_id,
        instructions: None,
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    schedule_refine_run(
        &session_id,
        &params,
        /*is_root*/ true,
        Some(temp.path()),
    );
    let pending = take_pending_refine(&session_id).unwrap();
    let permit = runtime.auxiliary_model_slots.acquire().await.unwrap();
    let path = ensure_session_harness(temp.path()).unwrap();
    // Old code returned a successful empty proposal after only 50ms. This
    // manual request must remain pending while the other auxiliary task owns
    // the slot, not silently claim that it inspected a clean harness.
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(150),
            runtime.plan_refine_proposal_llm(session_id, &pending, &path),
        )
        .await
        .is_err()
    );
    drop(permit);
    // This fixture has no model session. Once admitted, an unavailable model
    // must be an explicit failure rather than the old empty-proposal fallback.
    assert!(
        runtime
            .plan_refine_proposal_llm(session_id, &pending, &path)
            .await
            .is_err()
    );
}

async fn refine_test_session() -> (
    tempfile::TempDir,
    Arc<ServerRuntime>,
    u64,
    SessionId,
    tokio::sync::mpsc::Receiver<serde_json::Value>,
) {
    let temp = tempfile::TempDir::new().unwrap();
    let runtime = crate::test_support::TestRuntime::noop().runtime(temp.path());
    let (out, rx) = super::super::outbound::test_outbound_channel(128);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, out)
        .await;
    runtime
        .handle_acp_initialize(
            connection,
            Some(json!(1)),
            json!({
                "protocolVersion": 1, "clientCapabilities": { "terminal": false },
                "_meta": { "devo": { "protocol": "native" } }
            }),
        )
        .await;
    let created = runtime.handle_incoming(connection, json!({
        "id": 2, "method": "session/new", "params": { "cwd": temp.path(), "idempotencyKey": "refine-idle" }
    })).await.unwrap();
    let session_id: SessionId =
        serde_json::from_value(created["result"]["session"]["id"].clone()).unwrap();
    runtime
        .subscribe_connection_to_session(connection, session_id, None)
        .await;
    (temp, runtime, connection, session_id, rx)
}

#[tokio::test]
async fn prepared_refine_defers_across_missing_inline_then_commits_once() {
    let (temp, runtime, connection, session_id, mut rx) = refine_test_session().await;
    let params = SessionRefineRunParams {
        session_id,
        instructions: Some("Check git status before committing".into()),
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    schedule_refine_run(
        &session_id,
        &params,
        /*is_root*/ true,
        Some(temp.path()),
    );
    let mut pending = take_pending_refine(&session_id).unwrap();
    let path = ensure_session_harness(temp.path()).unwrap();
    pending.prepared = Some(Ok((path.clone(), plan_refine_proposal(&pending, &path))));
    PENDING_REFINES
        .lock()
        .unwrap()
        .insert(session_id.to_string(), pending);
    // Exact production finalization window: inline was taken for MergeTurn,
    // but the active stream has not yet been unregistered.
    runtime
        .register_active_stream(
            session_id,
            Arc::new(tokio::sync::Mutex::new(
                super::super::session_actor::state::SessionStreamState::default(),
            )),
        )
        .await;
    assert!(
        runtime
            .apply_pending_refine_at_boundary(
                session_id,
                &session_id,
                temp.path(),
                /*plan_mode*/ false
            )
            .await
            .is_none()
    );
    assert!(peek_pending_refine(&session_id));
    assert!(
        HarnessState::load(&path)
            .unwrap()
            .refinement_history
            .is_empty()
    );
    runtime.unregister_active_stream(session_id).await;
    // No model is available in this fixture: success proves the completed plan
    // survived deferral rather than rerunning planning or applying twice.
    let applied = runtime
        .apply_pending_refine_at_boundary(
            session_id,
            &session_id,
            temp.path(),
            /*plan_mode*/ false,
        )
        .await
        .unwrap();
    assert!(applied.failure.is_none());
    assert_eq!(applied.proposal.edits.len(), 1);
    assert!(!peek_pending_refine(&session_id));
    assert_eq!(
        HarnessState::load(&path).unwrap().refinement_history.len(),
        1
    );
    assert!(
        runtime
            .apply_pending_refine_at_boundary(
                session_id,
                &session_id,
                temp.path(),
                /*plan_mode*/ false
            )
            .await
            .is_none()
    );
    let notification = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event["method"] == "item/completed"
                && event["params"]["item"]["item"]["type"] == "refinement"
            {
                break event;
            }
        }
    })
    .await
    .unwrap();
    let history = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 8, "method": "session/items/list", "params": { "sessionId": session_id }
            }),
        )
        .await
        .unwrap();
    assert!(
        history["result"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == notification["params"]["item"]["id"])
    );
    assert_eq!(
        HarnessState::load(&path).unwrap().refinement_history.len(),
        1
    );
}

#[tokio::test]
async fn refine_requeue_recovers_boundary_that_already_observed_empty_pending() {
    let (temp, runtime, _connection, session_id, mut rx) = refine_test_session().await;
    let params = SessionRefineRunParams {
        session_id,
        instructions: None,
        global: false,
        rollback_id: None,
        trajectory_item_ids: Vec::new(),
    };
    schedule_refine_run(
        &session_id,
        &params,
        /*is_root*/ true,
        Some(temp.path()),
    );
    let mut pending = take_pending_refine(&session_id).unwrap();
    pending.prepared = Some(Err("controlled planner failure".into()));
    PENDING_REFINES
        .lock()
        .unwrap()
        .insert(session_id.to_string(), pending);
    runtime
        .register_active_stream(
            session_id,
            Arc::new(tokio::sync::Mutex::new(
                super::super::session_actor::state::SessionStreamState::default(),
            )),
        )
        .await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    REFINE_REQUEUE_TEST_GATES.lock().unwrap().insert(
        session_id.to_string(),
        RefineRequeueTestGate {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        },
    );
    let task_runtime = Arc::clone(&runtime);
    let dir = temp.path().to_path_buf();
    let task = tokio::spawn(async move {
        task_runtime
            .apply_pending_refine_at_boundary(
                session_id,
                &session_id,
                &dir,
                /*plan_mode*/ false,
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    // New turn finishes and its post-turn scheduler checks before the old
    // planner publishes prepared work. There will be no further turn to wake it.
    runtime.unregister_active_stream(session_id).await;
    assert!(
        runtime
            .apply_pending_refine_at_boundary(
                session_id,
                &session_id,
                temp.path(),
                /*plan_mode*/ false
            )
            .await
            .is_none()
    );
    release.notify_one();
    let applied = tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .expect("must self-drain after missed boundary");
    assert_eq!(
        applied.failure.as_deref(),
        Some("controlled planner failure")
    );
    assert!(!peek_pending_refine(&session_id));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event["method"] == "item/completed"
                && event["params"]["item"]["item"]["code"] == "refinement_not_applied"
            {
                break;
            }
        }
    })
    .await
    .unwrap();
}
