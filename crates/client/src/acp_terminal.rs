use std::collections::HashMap;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use devo_protocol::ACP_TERMINAL_CREATE_METHOD;
use devo_protocol::ACP_TERMINAL_KILL_METHOD;
use devo_protocol::ACP_TERMINAL_OUTPUT_METHOD;
use devo_protocol::ACP_TERMINAL_RELEASE_METHOD;
use devo_protocol::ACP_TERMINAL_WAIT_FOR_EXIT_METHOD;
use devo_protocol::AcpSuccessResponse;
use devo_protocol::AcpTerminalCreateParams;
use devo_protocol::AcpTerminalCreateResult;
use devo_protocol::AcpTerminalExitStatus;
use devo_protocol::AcpTerminalOutputResult;
use devo_protocol::AcpTerminalParams;
use devo_protocol::AcpTerminalWaitForExitResult;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::time::Duration;

use crate::stdio::ServerNotificationMessage;

static ACP_TERMINAL_NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub const ACP_TERMINAL_OUTPUT_NOTIFICATION_METHOD: &str = "_devo/acp_terminal/output";
const ACP_TERMINAL_DEFAULT_OUTPUT_BYTE_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Default)]
pub(crate) struct AcpTerminalManager {
    terminals: Arc<Mutex<HashMap<String, Arc<AcpTerminalHandle>>>>,
}

struct AcpTerminalHandle {
    state: Mutex<AcpTerminalState>,
    exit_notify: Notify,
}

struct AcpTerminalState {
    child: Child,
    output: String,
    truncated: bool,
    output_byte_limit: usize,
    exit_status: Option<AcpTerminalExitStatus>,
}

impl AcpTerminalManager {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    async fn insert(&self, terminal_id: String, terminal: Arc<AcpTerminalHandle>) {
        self.terminals.lock().await.insert(terminal_id, terminal);
    }

    async fn get(&self, terminal_id: &str) -> std::result::Result<Arc<AcpTerminalHandle>, String> {
        self.terminals
            .lock()
            .await
            .get(terminal_id)
            .cloned()
            .ok_or_else(|| format!("unknown terminalId {terminal_id}"))
    }

    pub(crate) async fn output(
        &self,
        terminal_id: &str,
    ) -> std::result::Result<AcpTerminalOutputResult, String> {
        acp_terminal_output(terminal_id, self.clone()).await
    }

    pub(crate) async fn release_all(&self) {
        let terminals = self.terminals.lock().await.drain().collect::<Vec<_>>();
        for (_, terminal) in terminals {
            let _ = kill_acp_terminal_if_running(&terminal).await;
        }
    }
}

pub(crate) async fn handle_acp_terminal_request(
    request_id: serde_json::Value,
    method: &str,
    params: serde_json::Value,
    terminals: AcpTerminalManager,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
) -> std::result::Result<serde_json::Value, String> {
    match method {
        ACP_TERMINAL_CREATE_METHOD => {
            let params = serde_json::from_value::<AcpTerminalCreateParams>(params)
                .map_err(|error| format!("invalid terminal/create params: {error}"))?;
            let terminal_id = create_acp_terminal(params, terminals, notifications_tx).await?;
            Ok(acp_terminal_success_response(
                request_id,
                AcpTerminalCreateResult {
                    terminal_id,
                    meta: None,
                },
            ))
        }
        ACP_TERMINAL_OUTPUT_METHOD => {
            let params = serde_json::from_value::<AcpTerminalParams>(params)
                .map_err(|error| format!("invalid terminal/output params: {error}"))?;
            let result = acp_terminal_output(&params.terminal_id, terminals).await?;
            Ok(acp_terminal_success_response(request_id, result))
        }
        ACP_TERMINAL_WAIT_FOR_EXIT_METHOD => {
            let params = serde_json::from_value::<AcpTerminalParams>(params)
                .map_err(|error| format!("invalid terminal/wait_for_exit params: {error}"))?;
            let status = wait_for_acp_terminal_exit(&params.terminal_id, terminals).await?;
            Ok(acp_terminal_success_response(
                request_id,
                AcpTerminalWaitForExitResult {
                    exit_code: status.exit_code,
                    signal: status.signal,
                    meta: None,
                },
            ))
        }
        ACP_TERMINAL_KILL_METHOD => {
            let params = serde_json::from_value::<AcpTerminalParams>(params)
                .map_err(|error| format!("invalid terminal/kill params: {error}"))?;
            kill_acp_terminal(&params.terminal_id, terminals).await?;
            Ok(acp_terminal_success_response(
                request_id,
                serde_json::json!({}),
            ))
        }
        ACP_TERMINAL_RELEASE_METHOD => {
            let params = serde_json::from_value::<AcpTerminalParams>(params)
                .map_err(|error| format!("invalid terminal/release params: {error}"))?;
            release_acp_terminal(&params.terminal_id, terminals).await?;
            Ok(acp_terminal_success_response(
                request_id,
                serde_json::json!({}),
            ))
        }
        _ => Err(format!("unknown ACP terminal method {method}")),
    }
}

async fn create_acp_terminal(
    params: AcpTerminalCreateParams,
    terminals: AcpTerminalManager,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
) -> std::result::Result<String, String> {
    if params.command.trim().is_empty() {
        return Err("terminal/create params.command must not be empty".to_string());
    }
    if let Some(cwd) = params.cwd.as_ref()
        && !cwd.is_absolute()
    {
        return Err("terminal/create params.cwd must be absolute".to_string());
    }

    let terminal_id = format!(
        "term_{}",
        ACP_TERMINAL_NEXT_ID.fetch_add(1, Ordering::SeqCst)
    );
    let mut command = Command::new(&params.command);
    command.args(params.args);
    for env in params.env {
        command.env(env.name, env.value);
    }
    if let Some(cwd) = params.cwd {
        command.current_dir(cwd);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn terminal command: {error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let terminal = Arc::new(AcpTerminalHandle {
        state: Mutex::new(AcpTerminalState {
            child,
            output: String::new(),
            truncated: false,
            output_byte_limit: params
                .output_byte_limit
                .unwrap_or(ACP_TERMINAL_DEFAULT_OUTPUT_BYTE_LIMIT),
            exit_status: None,
        }),
        exit_notify: Notify::new(),
    });
    terminals
        .insert(terminal_id.clone(), Arc::clone(&terminal))
        .await;

    if let Some(stdout) = stdout {
        tokio::spawn(read_acp_terminal_output(
            terminal_id.clone(),
            stdout,
            Arc::clone(&terminal),
            notifications_tx.clone(),
        ));
    }
    if let Some(stderr) = stderr {
        tokio::spawn(read_acp_terminal_output(
            terminal_id.clone(),
            stderr,
            Arc::clone(&terminal),
            notifications_tx,
        ));
    }
    tokio::spawn(watch_acp_terminal_exit(Arc::clone(&terminal)));
    Ok(terminal_id)
}

async fn read_acp_terminal_output<R>(
    terminal_id: String,
    mut reader: R,
    terminal: Arc<AcpTerminalHandle>,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut buffer = [0u8; 8192];
    let mut pending_utf8 = Vec::new();
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read_len) => {
                pending_utf8.extend_from_slice(&buffer[..read_len]);
                for delta in take_terminal_utf8_chunks(&mut pending_utf8) {
                    append_acp_terminal_output(&terminal, &delta).await;
                    let _ = notifications_tx.send(ServerNotificationMessage {
                        method: ACP_TERMINAL_OUTPUT_NOTIFICATION_METHOD.to_string(),
                        params: serde_json::json!({
                            "terminalId": terminal_id.clone(),
                            "delta": delta,
                        }),
                    });
                }
            }
            Err(error) => {
                tracing::debug!(%error, terminal_id, "failed to read ACP terminal output");
                break;
            }
        }
    }
    if !pending_utf8.is_empty() {
        let delta = String::from_utf8_lossy(&pending_utf8).to_string();
        append_acp_terminal_output(&terminal, &delta).await;
        let _ = notifications_tx.send(ServerNotificationMessage {
            method: ACP_TERMINAL_OUTPUT_NOTIFICATION_METHOD.to_string(),
            params: serde_json::json!({
                "terminalId": terminal_id,
                "delta": delta,
            }),
        });
    }
}

async fn watch_acp_terminal_exit(terminal: Arc<AcpTerminalHandle>) {
    loop {
        {
            let mut state = terminal.state.lock().await;
            match refresh_acp_terminal_exit_status(&mut state) {
                Ok(Some(_)) => {
                    terminal.exit_notify.notify_waiters();
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::debug!(%error, "failed to watch ACP terminal exit");
                    terminal.exit_notify.notify_waiters();
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn append_acp_terminal_output(terminal: &AcpTerminalHandle, delta: &str) {
    if delta.is_empty() {
        return;
    }
    let mut state = terminal.state.lock().await;
    state.output.push_str(delta);
    let output_byte_limit = state.output_byte_limit;
    if truncate_from_start_on_char_boundary(&mut state.output, output_byte_limit) {
        state.truncated = true;
    }
}

fn take_terminal_utf8_chunks(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut consumed = 0usize;
    while consumed < buffer.len() {
        match std::str::from_utf8(&buffer[consumed..]) {
            Ok(text) => {
                if !text.is_empty() {
                    chunks.push(text.to_string());
                }
                consumed = buffer.len();
                break;
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to > 0 {
                    let end = consumed + valid_up_to;
                    chunks.push(
                        std::str::from_utf8(&buffer[consumed..end])
                            .expect("valid UTF-8 prefix")
                            .to_string(),
                    );
                    consumed = end;
                }
                if let Some(error_len) = error.error_len() {
                    chunks.push("\u{FFFD}".to_string());
                    consumed += error_len;
                } else {
                    break;
                }
            }
        }
    }
    if consumed > 0 {
        buffer.drain(..consumed);
    }
    chunks
}

fn truncate_from_start_on_char_boundary(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }
    if limit == 0 {
        text.clear();
        return true;
    }
    let target = text.len().saturating_sub(limit);
    let split_index = text
        .char_indices()
        .find_map(|(index, _)| (index >= target).then_some(index))
        .unwrap_or(text.len());
    text.drain(..split_index);
    true
}

async fn acp_terminal_output(
    terminal_id: &str,
    terminals: AcpTerminalManager,
) -> std::result::Result<AcpTerminalOutputResult, String> {
    let terminal = terminals.get(terminal_id).await?;
    let mut state = terminal.state.lock().await;
    if refresh_acp_terminal_exit_status(&mut state)?.is_some() {
        terminal.exit_notify.notify_waiters();
    }
    Ok(AcpTerminalOutputResult {
        output: state.output.clone(),
        truncated: state.truncated,
        exit_status: state.exit_status.clone(),
        meta: None,
    })
}

async fn wait_for_acp_terminal_exit(
    terminal_id: &str,
    terminals: AcpTerminalManager,
) -> std::result::Result<AcpTerminalExitStatus, String> {
    let terminal = terminals.get(terminal_id).await?;
    loop {
        let notified = terminal.exit_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let mut state = terminal.state.lock().await;
            if let Some(status) = refresh_acp_terminal_exit_status(&mut state)? {
                return Ok(status);
            }
        }
        notified.await;
    }
}

async fn kill_acp_terminal(
    terminal_id: &str,
    terminals: AcpTerminalManager,
) -> std::result::Result<(), String> {
    let terminal = terminals.get(terminal_id).await?;
    kill_acp_terminal_if_running(&terminal).await
}

async fn release_acp_terminal(
    terminal_id: &str,
    terminals: AcpTerminalManager,
) -> std::result::Result<(), String> {
    let terminal = terminals
        .terminals
        .lock()
        .await
        .remove(terminal_id)
        .ok_or_else(|| format!("unknown terminalId {terminal_id}"))?;
    kill_acp_terminal_if_running(&terminal).await
}

async fn kill_acp_terminal_if_running(
    terminal: &AcpTerminalHandle,
) -> std::result::Result<(), String> {
    let mut state = terminal.state.lock().await;
    if refresh_acp_terminal_exit_status(&mut state)?.is_none()
        && let Err(error) = state.child.start_kill()
    {
        return Err(format!("failed to kill terminal: {error}"));
    }
    Ok(())
}

fn refresh_acp_terminal_exit_status(
    state: &mut AcpTerminalState,
) -> std::result::Result<Option<AcpTerminalExitStatus>, String> {
    if let Some(status) = state.exit_status.clone() {
        return Ok(Some(status));
    }
    let Some(status) = state
        .child
        .try_wait()
        .map_err(|error| format!("failed to query terminal exit status: {error}"))?
    else {
        return Ok(None);
    };
    let status = acp_terminal_exit_status_from_process_status(status);
    state.exit_status = Some(status.clone());
    Ok(Some(status))
}

fn acp_terminal_exit_status_from_process_status(status: ExitStatus) -> AcpTerminalExitStatus {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|signal| signal.to_string())
    };
    #[cfg(not(unix))]
    let signal = None;
    AcpTerminalExitStatus {
        exit_code: status.code(),
        signal,
    }
}

fn acp_terminal_success_response<T: serde::Serialize>(
    id: serde_json::Value,
    result: T,
) -> serde_json::Value {
    serde_json::to_value(AcpSuccessResponse::new(id, result))
        .expect("serialize ACP terminal success response")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc;
    use tokio::time::Duration;
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn acp_terminal_methods_run_command_and_release() {
        let session_id = devo_protocol::SessionId::new();
        let terminals = AcpTerminalManager::new();
        let (notifications_tx, mut notifications_rx) = mpsc::unbounded_channel();
        let (command, args) = short_terminal_command();
        let create_response = handle_acp_terminal_request(
            serde_json::json!(1),
            ACP_TERMINAL_CREATE_METHOD,
            serde_json::to_value(AcpTerminalCreateParams {
                session_id,
                command,
                args,
                env: Vec::new(),
                cwd: Some(std::env::current_dir().expect("current dir")),
                output_byte_limit: Some(128),
                meta: None,
            })
            .expect("serialize terminal/create params"),
            terminals.clone(),
            notifications_tx,
        )
        .await
        .expect("terminal/create succeeds");
        let create: AcpSuccessResponse<AcpTerminalCreateResult> =
            serde_json::from_value(create_response).expect("decode terminal/create response");
        let terminal_id = create.result.terminal_id;
        let terminal_params = AcpTerminalParams {
            session_id,
            terminal_id: terminal_id.clone(),
            meta: None,
        };

        let wait_response = timeout(
            Duration::from_secs(5),
            handle_acp_terminal_request(
                serde_json::json!(2),
                ACP_TERMINAL_WAIT_FOR_EXIT_METHOD,
                serde_json::to_value(&terminal_params).expect("serialize wait params"),
                terminals.clone(),
                mpsc::unbounded_channel().0,
            ),
        )
        .await
        .expect("terminal exits before timeout")
        .expect("terminal/wait_for_exit succeeds");
        let wait: AcpSuccessResponse<AcpTerminalWaitForExitResult> =
            serde_json::from_value(wait_response).expect("decode wait response");
        assert_eq!(wait.result.exit_code, Some(0));

        let output = timeout(Duration::from_secs(5), async {
            loop {
                let output = acp_terminal_output(&terminal_id, terminals.clone())
                    .await
                    .expect("terminal output exists");
                if output.output.contains("acp-terminal") {
                    return output;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("terminal output captured before timeout");
        assert!(!output.truncated);
        assert!(output.output.contains("acp-terminal"));

        let mut saw_output_notification = false;
        while let Ok(notification) = notifications_rx.try_recv() {
            saw_output_notification |= notification.method
                == ACP_TERMINAL_OUTPUT_NOTIFICATION_METHOD
                && notification.params.get("terminalId")
                    == Some(&serde_json::json!(terminal_id.clone()))
                && notification
                    .params
                    .get("delta")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|delta| delta.contains("acp-terminal"));
        }
        assert!(saw_output_notification);

        handle_acp_terminal_request(
            serde_json::json!(3),
            ACP_TERMINAL_KILL_METHOD,
            serde_json::to_value(&terminal_params).expect("serialize kill params"),
            terminals.clone(),
            mpsc::unbounded_channel().0,
        )
        .await
        .expect("terminal/kill succeeds");
        handle_acp_terminal_request(
            serde_json::json!(4),
            ACP_TERMINAL_RELEASE_METHOD,
            serde_json::to_value(&terminal_params).expect("serialize release params"),
            terminals.clone(),
            mpsc::unbounded_channel().0,
        )
        .await
        .expect("terminal/release succeeds");
        assert!(terminals.terminals.lock().await.is_empty());
        assert!(
            acp_terminal_output(&terminal_id, terminals)
                .await
                .expect_err("released terminal is removed")
                .contains("unknown terminalId")
        );
    }

    #[cfg(windows)]
    fn short_terminal_command() -> (String, Vec<String>) {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), "echo acp-terminal".to_string()],
        )
    }

    #[cfg(unix)]
    fn short_terminal_command() -> (String, Vec<String>) {
        (
            "sh".to_string(),
            vec!["-c".to_string(), "printf 'acp-terminal\\n'".to_string()],
        )
    }

    #[test]
    fn terminal_utf8_chunks_keep_split_character_boundary() {
        let mut buffer = vec![0xE2, 0x82];
        assert_eq!(take_terminal_utf8_chunks(&mut buffer), Vec::<String>::new());
        assert_eq!(buffer, vec![0xE2, 0x82]);

        buffer.extend([0xAC, b'\n']);
        assert_eq!(
            take_terminal_utf8_chunks(&mut buffer),
            vec!["€\n".to_string()]
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn truncate_keeps_valid_utf8_boundary() {
        let mut text = "a€b".to_string();
        assert!(truncate_from_start_on_char_boundary(&mut text, 2));
        assert_eq!(text, "b");
    }
}
