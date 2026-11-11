//! Projection of Python host-bash snapshots into the Native task read surface.
use devo_core::tools::host_bash::HostBashActivity;
use devo_protocol::native::ids::{ItemId, SessionId, TurnId};
use devo_protocol::native::item::{
    BackgroundTaskKind, Item, ItemEnvelope, ItemState, SpawnedWorkState,
};

pub(super) const PREFIX: &str = "host-bash:";

pub(super) fn item(snapshot: &HostBashActivity) -> ItemEnvelope {
    let session_id = SessionId::from_string(snapshot.session_id.clone());
    let (state, item_state) = if snapshot.running {
        (SpawnedWorkState::Running, ItemState::Running)
    } else if snapshot.cancelled {
        (SpawnedWorkState::Cancelled, ItemState::Interrupted)
    } else if snapshot.error.is_some() || snapshot.exit_code.is_some_and(|code| code != 0) {
        (SpawnedWorkState::Failed, ItemState::Failed)
    } else {
        (SpawnedWorkState::Completed, ItemState::Completed)
    };
    let output = if snapshot.output.is_empty() {
        snapshot.error.clone()
    } else {
        Some(snapshot.output.clone())
    };
    ItemEnvelope {
        id: ItemId::from_string(format!("{PREFIX}{}", snapshot.handle_id)),
        session_id,
        // Direct host callers can have no turn. Keep that explicit with a
        // synthetic namespace rather than claiming a session ID is a turn.
        turn_id: snapshot
            .turn_id
            .map(|id| TurnId::from_string(id.to_string()))
            .unwrap_or_else(|| {
                TurnId::from_string(format!("synthetic:host-bash:{}", snapshot.handle_id))
            }),
        seq: 0,
        revision: 1,
        created_at: snapshot.created_at,
        updated_at: snapshot.updated_at,
        state: item_state,
        item: Item::BackgroundTask {
            origin_call_id: None,
            task_kind: BackgroundTaskKind::Shell,
            state,
            // This observation is read-only. It must not be passed to the
            // process manager's stdin, resize or interrupt APIs.
            execution_handle: None,
            cwd: Some(snapshot.cwd.clone()),
            exit_code: snapshot.exit_code.and_then(|code| i32::try_from(code).ok()),
            command: Some(snapshot.command.clone()),
            output,
        },
        parent_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn host_bash_tasks_are_read_only_and_keep_terminal_failure() {
        let now = chrono::Utc::now();
        let snapshot = HostBashActivity {
            handle_id: "example".into(),
            session_id: "ses_example".into(),
            command: "echo hello".into(),
            turn_id: None,
            cancelled: false,
            cwd: std::path::PathBuf::from("workspace"),
            created_at: now,
            updated_at: now,
            running: false,
            exit_code: None,
            error: Some("cancelled".into()),
            output: String::new(),
        };
        let session_id = SessionId::from_string(snapshot.session_id.clone());
        assert_eq!(
            item(&snapshot),
            ItemEnvelope {
                id: ItemId::from_string("host-bash:example".into()),
                session_id,
                turn_id: TurnId::from_string("synthetic:host-bash:example".into()),
                seq: 0,
                revision: 1,
                created_at: now,
                updated_at: now,
                state: ItemState::Failed,
                item: Item::BackgroundTask {
                    origin_call_id: None,
                    task_kind: BackgroundTaskKind::Shell,
                    state: SpawnedWorkState::Failed,
                    execution_handle: None,
                    cwd: Some(snapshot.cwd),
                    exit_code: None,
                    command: Some(snapshot.command),
                    output: Some("cancelled".into())
                },
                parent_id: None,
            }
        );
    }
    #[test]
    fn exit_failure_cancellation_and_real_turn_are_preserved() {
        let now = chrono::Utc::now();
        let turn_id = devo_protocol::TurnId::new();
        let mut snapshot = HostBashActivity {
            handle_id: "states".into(),
            session_id: "ses_states".into(),
            command: "exit 7".into(),
            turn_id: Some(turn_id),
            cancelled: false,
            cwd: std::path::PathBuf::from("workspace"),
            created_at: now,
            updated_at: now,
            running: false,
            exit_code: Some(7),
            error: None,
            output: String::new(),
        };
        let failed = item(&snapshot);
        assert_eq!(failed.turn_id, TurnId::from_string(turn_id.to_string()));
        assert_eq!(failed.state, ItemState::Failed);
        assert!(matches!(
            failed.item,
            Item::BackgroundTask {
                state: SpawnedWorkState::Failed,
                ..
            }
        ));
        snapshot.cancelled = true;
        let cancelled = item(&snapshot);
        assert_eq!(cancelled.state, ItemState::Interrupted);
        assert!(matches!(
            cancelled.item,
            Item::BackgroundTask {
                state: SpawnedWorkState::Cancelled,
                ..
            }
        ));
    }
}
