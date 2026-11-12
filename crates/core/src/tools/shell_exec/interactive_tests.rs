use super::{InteractiveIo, ShellExecRequest, execute_shell_command};
use base64::Engine;
use pretty_assertions::assert_eq;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cursor_retains_recent_binary_bytes_and_reports_gaps() {
    let io = InteractiveIo::default();
    io.append(&vec![b'x'; 256 * 1024 + 10], "stdout");
    io.append(&[0xff, 0x00, 0xe4, 0xb8, 0xad], "stderr");
    io.finish();
    let first = io.read(0, 4, 0).await;
    assert_eq!(
        (first["truncated"].as_bool(), first["next_cursor"].as_u64()),
        (Some(true), Some(19))
    );
    let last = io.read((256 * 1024 + 10) as u64, 10, 0).await;
    assert_eq!(
        (last["eof"].as_bool(), last["data"].as_str()),
        (Some(true), Some("/wDkuK0="))
    );
    assert_eq!(last["chunks"][0]["stream"], "stderr");
    assert!(io.read(u64::MAX, 1, 0).await.get("error").is_some());
}

#[tokio::test]
async fn read_wait_is_not_process_termination_and_failed_launch_wakes_ready() {
    let io = InteractiveIo::default();
    let read = io.read(0, 10, 1).await;
    assert_eq!(
        (read["data"].as_str(), read["eof"].as_bool()),
        (Some(""), Some(false))
    );
    io.finish();
    assert!(io.ready().await.is_err());
    assert!(io.write(vec![0; 65537]).await.is_err());
}

fn request(command: &str, io: InteractiveIo, tty: bool) -> ShellExecRequest {
    ShellExecRequest {
        output_capture: None,
        command: command.into(),
        sandbox_workspace: None,
        workdir: std::env::temp_dir(),
        description: "interactive regression".into(),
        shell_override: None,
        tty,
        login: false,
        timeout_ms: 10_000,
        yield_time_ms: 10,
        max_output_tokens: 1000,
        progress_limit_bytes: Some(256 * 1024),
        output_tail: None,
        interactive: Some(io),
        sandbox_profile: Some("off".into()),
        sandbox_permission_overlay: None,
    }
}

async fn read_until(io: &InteractiveIo, marker: &[u8]) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut output = Vec::new();
        let mut cursor = 0;
        loop {
            let read = io.read(cursor, 65536, 100).await;
            cursor = read["next_cursor"].as_u64().unwrap();
            output.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(read["data"].as_str().unwrap())
                    .unwrap(),
            );
            if output.windows(marker.len()).any(|bytes| bytes == marker) {
                return output;
            }
            assert_ne!(read["eof"], true, "ended before marker: {output:?}");
        }
    })
    .await
    .expect("output deadline")
}

#[tokio::test]
async fn real_pipe_accepts_multiple_inputs_and_eof() {
    #[cfg(unix)]
    let command = "while IFS= read -r line; do printf 'reply:%s\\n' \"$line\"; done; printf eof";
    #[cfg(windows)]
    let command = "while ($null -ne ($line = [Console]::ReadLine())) { [Console]::WriteLine('reply:' + $line) }; [Console]::Write('eof')";
    let io = InteractiveIo::default();
    let task = tokio::spawn(execute_shell_command(
        request(command, io.clone(), /*tty*/ false),
        None,
        CancellationToken::new(),
    ));
    tokio::time::timeout(Duration::from_secs(8), io.ready())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(io.write(b"one\n".to_vec()).await.unwrap(), 4);
    read_until(&io, b"reply:one").await;
    io.write(b"two\n".to_vec()).await.unwrap();
    read_until(&io, b"reply:two").await;
    io.close_stdin().await.unwrap();
    assert!(!task.await.unwrap().unwrap().is_error);
    read_until(&io, b"eof").await;
    assert!(io.write(b"late".to_vec()).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn real_pty_writes_resizes_and_cancels() {
    let io = InteractiveIo::new(/*tty*/ true);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(execute_shell_command(
        request(
            "printf ready; read -r line; printf 'reply:%s\\n' \"$line\"; sleep 30",
            io.clone(),
            true,
        ),
        None,
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(8), io.ready())
        .await
        .unwrap()
        .unwrap();
    io.resize(90, 32).await.unwrap();
    assert!(io.close_stdin().await.is_err());
    io.write(b"hello\n".to_vec()).await.unwrap();
    read_until(&io, b"reply:hello").await;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.is_error);
}

#[tokio::test]
async fn readers_have_independent_cursors_and_wake_on_new_data() {
    let io = InteractiveIo::default();
    let waiting_io = io.clone();
    let pending = tokio::spawn(async move { waiting_io.read(0, 32, 1000).await });
    tokio::task::yield_now().await;
    io.append(b"first", "stdout");
    let first = pending.await.unwrap();
    assert_eq!(first["data"], "Zmlyc3Q=");
    assert_eq!(io.read(0, 32, 0).await["data"], first["data"]);
    io.append(b"second", "stderr");
    let second = io.read(5, 32, 0).await;
    assert_eq!(second["data"], "c2Vjb25k");
    assert_eq!(second["chunks"][0]["stream"], "stderr");
    io.finish();
    assert_eq!(io.read(11, 32, 0).await["eof"], true);
    io.append(b"late", "stdout");
    assert_eq!(io.read(11, 32, 0).await["data"], "");
}

#[cfg(unix)]
#[tokio::test]
async fn real_pty_continuous_output_remains_cancellable() {
    let io = InteractiveIo::new(/*tty*/ true);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(execute_shell_command(
        request(
            "while :; do printf 'continuous-output\\n'; done",
            io.clone(),
            true,
        ),
        None,
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(8), io.ready())
        .await
        .unwrap()
        .unwrap();
    read_until(&io, b"continuous-output").await;
    cancel.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .is_error
    );
}

#[cfg(unix)]
#[tokio::test]
async fn blocked_pipe_input_reports_uncertain_delivery_without_hanging_task_cleanup() {
    let io = InteractiveIo::default();
    let cancel = CancellationToken::new();
    let task = tokio::spawn(execute_shell_command(
        request("sleep 30", io.clone(), /*tty*/ false),
        None,
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(8), io.ready())
        .await
        .unwrap()
        .unwrap();
    let writing_io = io.clone();
    let writing = tokio::spawn(async move {
        // Fill platform pipe capacity; the child deliberately never reads.
        for _ in 0..32 {
            if let Err(error) = writing_io.write(vec![b'x'; 65536]).await {
                return error;
            }
        }
        panic!("test did not saturate the pipe");
    });
    let error = tokio::time::timeout(Duration::from_secs(7), writing)
        .await
        .unwrap()
        .unwrap();
    assert!(error.contains("partial"), "{error}");
    cancel.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .is_error
    );
}
