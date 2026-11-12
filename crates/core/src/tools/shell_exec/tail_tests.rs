use super::*;
use crate::tools::host_bash::OutputTail;

#[cfg(unix)]
#[tokio::test]
async fn recent_output_continues_after_progress_and_final_capture_limits() {
    let tail = OutputTail::default();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = execute_shell_command(
        ShellExecRequest {
            output_capture: None,
            // Exceed both progress (256 KiB) and final pipe capture (1 MiB).
            command: "head -c 2097152 /dev/zero | tr '\\0' x; printf '\\nLATEST_MARKER\\n'".into(),
            workdir: std::env::current_dir().unwrap(),
            sandbox_workspace: None,
            description: "rolling tail regression".into(),
            shell_override: Some("bash".into()),
            tty: false,
            login: false,
            timeout_ms: 10_000,
            yield_time_ms: 100,
            max_output_tokens: 100,
            progress_limit_bytes: Some(256 * 1024),
            output_tail: Some(tail.clone()),
            sandbox_profile: None,
            sandbox_permission_overlay: None,
        },
        Some(sender),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!result.is_error);
    let mut progress = String::new();
    while let Some(delta) = receiver.recv().await {
        progress.push_str(&delta);
    }
    assert!(
        !progress.contains("LATEST_MARKER"),
        "progress must remain capped"
    );
    assert!(tail.snapshot(256 * 1024).ends_with("\nLATEST_MARKER\n"));
    assert!(tail.snapshot(256 * 1024).len() <= 256 * 1024);
}
