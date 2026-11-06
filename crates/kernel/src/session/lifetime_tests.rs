//! Kernel-owned host work must end with the kernel, not the active turn.

use super::KernelSession;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

async fn fake_kernel() -> Arc<KernelSession> {
    let mut command = Command::new("python3");
    command.arg("-u").arg("-c").arg(
        r#"
import json, sys
print(json.dumps({"event":"ready","protocol":3,"python":"fake"}), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    if req.get("code") == "exit":
        break
    print(json.dumps({"event":"done","id":req["id"],"status":"ok"}), flush=True)
    if req.get("type") == "shutdown":
        break
"#,
    );
    KernelSession::spawn_command(command, None)
        .await
        .expect("fake kernel")
}

#[tokio::test]
async fn lifetime_cancels_on_shutdown_and_terminate() {
    let kernel = fake_kernel().await;
    let token = kernel.lifetime_token();
    let independent = kernel.lifetime_token();
    independent.cancel();
    assert_eq!(token.is_cancelled(), false);
    kernel.execute("cell").await.expect("cell");
    assert_eq!(token.is_cancelled(), false);
    kernel.shutdown().await.expect("shutdown");
    assert_eq!(token.is_cancelled(), true);

    let kernel = fake_kernel().await;
    let token = kernel.lifetime_token();
    kernel.terminate().await;
    assert_eq!(token.is_cancelled(), true);
}

#[tokio::test]
async fn lifetime_cancels_on_eof_and_drop() {
    let kernel = fake_kernel().await;
    let token = kernel.lifetime_token();
    assert!(kernel.execute("exit").await.is_err());
    tokio::time::timeout(Duration::from_secs(2), token.cancelled())
        .await
        .expect("EOF cancellation");

    let kernel = fake_kernel().await;
    let token = kernel.lifetime_token();
    drop(kernel);
    assert_eq!(token.is_cancelled(), true);
}

#[tokio::test]
async fn eof_cancels_lifetime_while_host_handler_is_pending() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let started = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let notified = Arc::clone(&started);
    let observed = Arc::clone(&dropped);
    let handler: super::HostRequestHandler = Arc::new(move |_, _| {
        let notified = Arc::clone(&notified);
        let observed = Arc::clone(&observed);
        Box::pin(async move {
            let _drop = Dropped(observed);
            notified.notify_one();
            std::future::pending().await
        })
    });
    let mut command = Command::new("python3");
    command.arg("-u").arg("-c").arg(
        r#"
import json, sys
print(json.dumps({"event":"ready","protocol":3,"python":"fake"}), flush=True)
req = json.loads(sys.stdin.readline())
print(json.dumps({"event":"host_request","id":"pending","cell_id":req["id"],"data":{}}), flush=True)
# Exit only after the test has observed the blocked callback.
sys.stdin.readline()
"#,
    );
    let kernel = KernelSession::spawn_command(command, Some(handler))
        .await
        .expect("fake kernel");
    let token = kernel.lifetime_token();
    let execution_kernel = Arc::clone(&kernel);
    let execution = tokio::spawn(async move { execution_kernel.execute("start").await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .expect("handler started");
    {
        use tokio::io::AsyncWriteExt;
        kernel
            .io
            .stdin
            .lock()
            .await
            .write_all(b"exit\n")
            .await
            .expect("release fake");
    }
    tokio::time::timeout(Duration::from_secs(2), token.cancelled())
        .await
        .expect("EOF cancellation");
    assert!(execution.await.expect("join").is_err());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("handler canceled");
}
