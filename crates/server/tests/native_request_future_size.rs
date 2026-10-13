use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use tempfile::TempDir;

#[path = "support/goal_continuation.rs"]
mod support;

#[tokio::test]
async fn native_request_futures_fit_a_small_caller_stack_budget() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = support::build_runtime(
        data_root.path(),
        Arc::new(support::PendingProvider::default()),
    )?;
    let sizes = [
        future_size(|| runtime.handle_incoming(0, serde_json::Value::Null)),
        future_size(|| runtime.handle_incoming_with_actions(0, serde_json::Value::Null)),
    ];
    println!("Native dispatch caller storage: {sizes:?} bytes");
    assert!(
        sizes.iter().all(|size| *size <= 2048),
        "Native dispatch must not embed its full handler graph in callers: {sizes:?} bytes"
    );
    Ok(())
}

// Infer the future type without constructing the oversized baseline future on
// the test thread. This measures caller storage, not an allocator's heap size.
fn future_size<F: Future>(_factory: impl FnOnce() -> F) -> usize {
    std::mem::size_of::<F>()
}
