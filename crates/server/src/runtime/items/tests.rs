use std::sync::Arc;

use pretty_assertions::assert_eq;
use tokio::sync::Mutex;

use super::*;
use crate::runtime::session_actor::state::SessionStreamState;
use crate::test_support::TestRuntime;

#[tokio::test]
async fn missing_session_keeps_default_item_sequence() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let runtime = TestRuntime::noop().runtime(directory.path());

    assert_eq!(
        runtime
            .allocate_item_sequence(&NativeSessionId::new())
            .await,
        1
    );
}

#[tokio::test]
async fn active_stream_without_inline_state_keeps_fallback_item_sequence() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let runtime = TestRuntime::noop().runtime(directory.path());
    let session_id = NativeSessionId::new();
    runtime
        .active_turns
        .register_stream(
            session_id,
            Arc::new(Mutex::new(SessionStreamState::default())),
        )
        .await;

    let first = runtime.allocate_item_sequence(&session_id).await;
    let second = runtime.allocate_item_sequence(&session_id).await;
    assert!(first >= 1 << 32);
    assert!(second > first);

    runtime.active_turns.unregister_stream(session_id).await;
    assert_eq!(runtime.allocate_item_sequence(&session_id).await, 1);
}
