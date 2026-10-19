use super::*;
use crate::runtime::compact_host::{
    clear_pending_compact, pending_compact_request, schedule_compact_run,
};
use pretty_assertions::assert_eq;

enum RequestChange {
    Cancel,
    Replace,
}

async fn stale_attempt_never_starts(change: RequestChange) -> Result<()> {
    let env = NativeEnv::connect().await?;
    let session_id =
        start_durable_session(&env.runtime, env.connection_id, env.data_root.path()).await?;
    schedule_compact_run(
        &session_id,
        Some("old instructions".into()),
        /*turn_active*/ true,
    );
    let attempted = pending_compact_request(&session_id).expect("scheduled request");
    // Reproduce a scheduler that already read its request before another task
    // cancels/replaces it. Admission must revalidate identity, not just cleanup.
    match change {
        RequestChange::Cancel => clear_pending_compact(&session_id),
        RequestChange::Replace => {
            schedule_compact_run(
                &session_id,
                Some("new instructions".into()),
                /*turn_active*/ true,
            );
        }
    }
    let expected_pending = pending_compact_request(&session_id);
    env.runtime
        .execute_agent_requested_compaction(session_id, attempted)
        .await;
    let handle = env.runtime.session(session_id).await.expect("session");
    let reservation = handle.turn_reservation_snapshot().await.expect("snapshot");
    assert_eq!(
        (
            reservation.active_turn.map(|turn| turn.native.kind),
            reservation.latest_turn.map(|turn| turn.native.kind),
        ),
        (None, None),
        "a stale compaction request must not create a turn"
    );
    assert_eq!(pending_compact_request(&session_id), expected_pending);
    clear_pending_compact(&session_id);
    Ok(())
}

#[tokio::test]
async fn canceled_compaction_attempt_is_not_admitted() -> Result<()> {
    stale_attempt_never_starts(RequestChange::Cancel).await
}

#[tokio::test]
async fn replaced_compaction_attempt_is_not_admitted() -> Result<()> {
    stale_attempt_never_starts(RequestChange::Replace).await
}
