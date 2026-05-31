use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::ProviderVendorCatalog;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader as AsyncBufReader;
use tokio::process::Command;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolRegistry;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use futures::stream;

const STDIO_SERVER_LINE_TIMEOUT: Duration = Duration::from_secs(30);

fn write_test_config(home_dir: &TempDir, listen: &[&str]) -> Result<()> {
    let config_dir = home_dir.path().join(".devo");

    std::fs::create_dir_all(&config_dir)?;
    let listen_entries = listen
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let config = format!(
        "[server]\nlisten = [{listen_entries}]\nmax_connections = 32\nevent_buffer_size = 128\nidle_session_timeout_secs = 300\npersist_ephemeral_sessions = false\n\n[defaults]\nmodel_binding = \"test-openai\"\n\n[providers.openai]\nenabled = true\nname = \"OpenAI\"\nwire_apis = [\"openai_chat_completions\"]\n\n[model_bindings.test-openai]\nenabled = true\nmodel_slug = \"test-model\"\nprovider = \"openai\"\nmodel_name = \"test-model\"\ninvocation_method = \"openai_chat_completions\"\n"
    );
    std::fs::write(config_dir.join("config.toml"), config)?;
    Ok(())
}

fn initialize_request(transport: &str) -> serde_json::Value {
    serde_json::json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "client_name": "e2e-test",
            "client_version": "1.0.0",
            "transport": transport,
            "supports_streaming": true,
            "supports_binary_images": false,
            "opt_out_notification_methods": [],
        }
    })
}

struct PendingProvider;

#[async_trait]
impl ModelProviderSDK for PendingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("test provider does not support completion")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::pending()))
    }

    fn name(&self) -> &str {
        "pending-test-provider"
    }
}

struct StreamingToolProvider {
    requests: AtomicUsize,
    workspace: PathBuf,
}

impl StreamingToolProvider {
    fn new(workspace: PathBuf) -> Self {
        Self {
            requests: AtomicUsize::new(0),
            workspace,
        }
    }
}

#[async_trait]
impl ModelProviderSDK for StreamingToolProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("test provider does not support completion")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst);
        let read_input = serde_json::json!({
            "filePath": self.workspace.join("README.md").to_string_lossy().to_string()
        });
        let glob_input = serde_json::json!({
            "pattern": "**/Cargo.toml",
            "path": "crates"
        });

        let events = if request_number == 0 {
            vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "read-1".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({}),
                }),
                Ok(StreamEvent::ToolCallStart {
                    index: 1,
                    id: "glob-1".to_string(),
                    name: "glob".to_string(),
                    input: serde_json::json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: read_input.to_string(),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 1,
                    partial_json: glob_input.to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-tools".to_string(),
                        content: vec![
                            ResponseContent::ToolUse {
                                id: "read-1".to_string(),
                                name: "read".to_string(),
                                input: serde_json::json!({}),
                            },
                            ResponseContent::ToolUse {
                                id: "glob-1".to_string(),
                                name: "glob".to_string(),
                                input: serde_json::json!({}),
                            },
                        ],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ]
        } else {
            vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "done".to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-done".to_string(),
                        content: vec![ResponseContent::Text("done".to_string())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ]
        };

        Ok(Box::pin(stream::iter(events)))
    }

    fn name(&self) -> &str {
        "streaming-tool-test-provider"
    }
}

#[tokio::test]
async fn stdio_server_process_supports_handshake_and_session_start() -> Result<()> {
    let home_dir = TempDir::new()?;
    write_test_config(&home_dir, &["stdio://"])?;

    let test_cwd = home_dir.path().to_string_lossy().into_owned();

    let mut command = devo_command()?;
    let mut child = command
        .arg("server")
        .arg("--transport")
        .arg("stdio")
        .env("DEVO_HOME", home_dir.path().join(".devo"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("spawn devo child process in server mode")?;

    let mut stdin = child.stdin.take().context("capture child stdin")?;
    let stdout = child.stdout.take().context("capture child stdout")?;
    let stderr = child.stderr.take().context("capture child stderr")?;
    let mut stdout_reader = AsyncBufReader::new(stdout).lines();
    let mut stderr_reader = AsyncBufReader::new(stderr);

    stdin
        .write_all(format!("{}\n", initialize_request("stdio")).as_bytes())
        .await?;
    stdin.flush().await?;

    let line = read_stdio_line(&mut stdout_reader, "initialize response").await?;
    let initialize_response: serde_json::Value =
        parse_stdio_json_line(&mut child, &mut stderr_reader, "initialize response", &line).await?;
    assert_eq!(initialize_response["id"], serde_json::json!(1));
    assert_eq!(
        initialize_response["result"]["server_name"],
        serde_json::json!("devo-server")
    );

    stdin.write_all(b"{\"method\":\"initialized\"}\n").await?;
    stdin
        .write_all(
            format!(
                "{}\n",
                serde_json::json!({
                    "id": 2,
                    "method": "session/start",
                    "params": {
                        "cwd": test_cwd,
                        "ephemeral": false,
                        "title": "End To End",
                        "model": "test-model"
                    }
                })
            )
            .as_bytes(),
        )
        .await?;
    stdin.flush().await?;

    let first_message =
        read_stdio_line(&mut stdout_reader, "first post-session/start message").await?;
    let second_message =
        read_stdio_line(&mut stdout_reader, "second post-session/start message").await?;

    let first_value = parse_stdio_json_line(
        &mut child,
        &mut stderr_reader,
        "first post-session/start message",
        &first_message,
    )
    .await?;
    let second_value = parse_stdio_json_line(
        &mut child,
        &mut stderr_reader,
        "second post-session/start message",
        &second_message,
    )
    .await?;
    let messages = [first_value, second_value];

    let session_started = messages
        .iter()
        .find(|value| value.get("method") == Some(&serde_json::json!("session/started")))
        .context("find session/started notification")?;
    let session_start_response = messages
        .iter()
        .find(|value| value.get("id") == Some(&serde_json::json!(2)))
        .context("find session/start response")?;

    assert_eq!(
        session_started["params"]["session"]["cwd"],
        serde_json::json!(test_cwd)
    );
    assert_eq!(
        session_start_response["result"]["session"]["model"],
        serde_json::json!("test-model")
    );

    drop(stdin);
    child.kill().await.ok();
    let _ = child.wait().await;
    Ok(())
}

#[tokio::test]
async fn websocket_listener_supports_handshake_subscription_and_turn_lifecycle() -> Result<()> {
    let port = {
        let listener = StdTcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        port
    };
    let bind_address = format!("127.0.0.1:{port}");
    let db_path = std::env::temp_dir().join("test_end_to_end.db");
    let db = Arc::new(devo_server::db::Database::open(db_path).expect("open test database"));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(PendingProvider);
    let runtime = ServerRuntime::new(
        std::env::temp_dir(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::default()),
            Arc::new(ProviderVendorCatalog::default()),
            None,
            Box::new(FileSystemSkillCatalog::new(SkillsConfig::default())),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(
                AppConfigStore::load(std::env::temp_dir(), None).expect("load app config store"),
            )),
        ),
    );
    let listen = vec![format!("ws://{bind_address}")];
    let listener_task =
        tokio::spawn(
            async move { devo_server::run_listeners(Arc::clone(&runtime), &listen).await },
        );

    tokio::time::sleep(Duration::from_millis(200)).await;

    let (mut socket, _) = connect_async(format!("ws://{bind_address}")).await?;
    socket
        .send(Message::Text(
            serde_json::to_string(&initialize_request("web_socket"))?.into(),
        ))
        .await?;

    let initialize_response = read_websocket_json(&mut socket).await?;
    assert_eq!(initialize_response["id"], serde_json::json!(1));
    assert_eq!(
        initialize_response["result"]["server_name"],
        serde_json::json!("devo-server")
    );

    socket
        .send(Message::Text(
            serde_json::json!({ "method": "initialized" })
                .to_string()
                .into(),
        ))
        .await?;

    socket
        .send(Message::Text(
            serde_json::json!({
                "id": 2,
                "method": "session/start",
                "params": {
                    "cwd": "C:/repo",
                    "ephemeral": false,
                    "title": null,
                    "model": "test-model"
                }
            })
            .to_string()
            .into(),
        ))
        .await?;

    let session_start_messages = read_n_websocket_json(&mut socket, 2).await?;
    let session_started = session_start_messages
        .iter()
        .find(|value| value.get("method") == Some(&serde_json::json!("session/started")))
        .context("find session/started notification")?;
    let session_response = session_start_messages
        .iter()
        .find(|value| value.get("id") == Some(&serde_json::json!(2)))
        .context("find session/start response")?;
    let session_id = session_response["result"]["session"]["session_id"]
        .as_str()
        .context("extract session id")?
        .to_string();
    assert_eq!(
        session_started["params"]["session"]["session_id"],
        serde_json::json!(session_id)
    );

    socket
        .send(Message::Text(
            serde_json::json!({
                "id": 3,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "hello" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            })
            .to_string()
            .into(),
        ))
        .await?;

    let turn_start_messages = read_until_websocket_json(
        &mut socket,
        |messages| {
            messages
                .iter()
                .any(|value| value.get("method") == Some(&serde_json::json!("turn/started")))
                && messages
                    .iter()
                    .any(|value| value.get("id") == Some(&serde_json::json!(3)))
        },
        4,
    )
    .await
    .context("read turn/start websocket messages")?;
    let turn_started = turn_start_messages
        .iter()
        .find(|value| value.get("method") == Some(&serde_json::json!("turn/started")))
        .context("find turn/started notification")?;
    let turn_start_response = turn_start_messages
        .iter()
        .find(|value| value.get("id") == Some(&serde_json::json!(3)))
        .context("find turn/start response")?;
    let turn_id = turn_start_response["result"]["turn_id"]
        .as_str()
        .context("extract turn id")?
        .to_string();
    assert_eq!(
        turn_started["params"]["turn"]["turn_id"],
        serde_json::json!(turn_id)
    );

    socket
        .send(Message::Text(
            serde_json::json!({
                "id": 4,
                "method": "turn/interrupt",
                "params": {
                    "session_id": session_id,
                    "turn_id": turn_id,
                    "reason": "e2e test"
                }
            })
            .to_string()
            .into(),
        ))
        .await?;

    let interrupt_messages = read_until_websocket_json(
        &mut socket,
        |messages| {
            messages
                .iter()
                .any(|value| value.get("id") == Some(&serde_json::json!(4)))
                && messages.iter().any(|value| {
                    value.get("method") == Some(&serde_json::json!("turn/interrupted"))
                })
                && messages
                    .iter()
                    .any(|value| value.get("method") == Some(&serde_json::json!("turn/completed")))
        },
        8,
    )
    .await
    .context("read turn/interrupt websocket messages")?;
    let interrupt_response = interrupt_messages
        .iter()
        .find(|value| value.get("id") == Some(&serde_json::json!(4)))
        .context("find turn/interrupt response")?;
    let interrupted_event = interrupt_messages
        .iter()
        .find(|value| value.get("method") == Some(&serde_json::json!("turn/interrupted")))
        .context("find turn/interrupted notification")?;
    let completed_event = interrupt_messages
        .iter()
        .find(|value| value.get("method") == Some(&serde_json::json!("turn/completed")))
        .context("find turn/completed notification")?;

    assert_eq!(
        interrupt_response["result"]["status"],
        serde_json::json!("Interrupted")
    );
    assert_eq!(
        interrupted_event["params"]["turn"]["status"],
        serde_json::json!("Interrupted")
    );
    assert_eq!(
        completed_event["params"]["turn"]["status"],
        serde_json::json!("Interrupted")
    );

    listener_task.abort();
    let _ = listener_task.await;
    Ok(())
}

#[tokio::test]
async fn websocket_turn_streams_final_tool_metadata_for_read_and_glob() -> Result<()> {
    let workspace = TempDir::new()?;
    std::fs::write(workspace.path().join("README.md"), "# Test\n")?;
    std::fs::create_dir_all(workspace.path().join("crates/tools"))?;
    std::fs::write(
        workspace.path().join("crates/tools/Cargo.toml"),
        "[package]\nname = \"tools\"\n",
    )?;

    let port = {
        let listener = StdTcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        port
    };
    let bind_address = format!("127.0.0.1:{port}");
    let db_dir = TempDir::new()?;
    let db = Arc::new(devo_server::db::Database::open(
        db_dir.path().join("e2e.db"),
    )?);
    let provider: Arc<dyn ModelProviderSDK> =
        Arc::new(StreamingToolProvider::new(workspace.path().to_path_buf()));
    let runtime = ServerRuntime::new(
        workspace.path().to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(devo_core::tools::create_default_tool_registry()),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::default()),
            Arc::new(ProviderVendorCatalog::default()),
            None,
            Box::new(FileSystemSkillCatalog::new(SkillsConfig::default())),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(
                AppConfigStore::load(std::env::temp_dir(), None).expect("load app config store"),
            )),
        ),
    );
    let listen = vec![format!("ws://{bind_address}")];
    let listener_task =
        tokio::spawn(
            async move { devo_server::run_listeners(Arc::clone(&runtime), &listen).await },
        );

    tokio::time::sleep(Duration::from_millis(200)).await;

    let (mut socket, _) = connect_async(format!("ws://{bind_address}")).await?;
    socket
        .send(Message::Text(
            serde_json::to_string(&initialize_request("web_socket"))?.into(),
        ))
        .await?;
    let initialize_response = read_websocket_json(&mut socket).await?;
    assert_eq!(initialize_response["id"], serde_json::json!(1));

    socket
        .send(Message::Text(
            serde_json::json!({ "method": "initialized" })
                .to_string()
                .into(),
        ))
        .await?;
    socket
        .send(Message::Text(
            serde_json::json!({
                "id": 2,
                "method": "session/start",
                "params": {
                    "cwd": workspace.path().to_string_lossy(),
                    "ephemeral": false,
                    "title": null,
                    "model": "test-model"
                }
            })
            .to_string()
            .into(),
        ))
        .await?;

    let session_start_messages = read_n_websocket_json(&mut socket, 2).await?;
    let session_response = session_start_messages
        .iter()
        .find(|value| value.get("id") == Some(&serde_json::json!(2)))
        .context("find session/start response")?;
    let session_id = session_response["result"]["session"]["session_id"]
        .as_str()
        .context("extract session id")?
        .to_string();

    socket
        .send(Message::Text(
            serde_json::json!({
                "id": 3,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "read and glob" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            })
            .to_string()
            .into(),
        ))
        .await?;

    let messages = read_until_websocket_json(
        &mut socket,
        |messages| {
            messages
                .iter()
                .any(|value| value.get("method") == Some(&serde_json::json!("turn/completed")))
        },
        80,
    )
    .await
    .context("read turn lifecycle messages")?;

    let completed_tool_calls = messages
        .iter()
        .filter(|value| {
            value.get("method") == Some(&serde_json::json!("item/completed"))
                && value["params"]["item"]["item_kind"] == serde_json::json!("tool_call")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        completed_tool_calls.len(),
        2,
        "expected completed ToolCall items: {messages:#?}"
    );

    let read_call = completed_tool_calls
        .iter()
        .find(|value| value["params"]["item"]["payload"]["tool_name"] == serde_json::json!("read"))
        .context("find read tool call")?;
    assert_eq!(
        read_call["params"]["item"]["payload"]["parameters"]["filePath"],
        serde_json::json!(
            workspace
                .path()
                .join("README.md")
                .to_string_lossy()
                .to_string()
        )
    );
    assert_eq!(
        read_call["params"]["item"]["payload"]["command_actions"][0]["name"],
        serde_json::json!("README.md")
    );

    let glob_call = completed_tool_calls
        .iter()
        .find(|value| value["params"]["item"]["payload"]["tool_name"] == serde_json::json!("glob"))
        .context("find glob tool call")?;
    assert_eq!(
        glob_call["params"]["item"]["payload"]["parameters"]["pattern"],
        serde_json::json!("**/Cargo.toml")
    );
    assert_eq!(
        glob_call["params"]["item"]["payload"]["command_actions"][0]["path"],
        serde_json::json!("**/Cargo.toml in crates")
    );

    listener_task.abort();
    let _ = listener_task.await;
    Ok(())
}

fn devo_command() -> Result<Command> {
    if let Some(binary_path) = std::env::var_os("CARGO_BIN_EXE_devo").map(PathBuf::from)
        && binary_path.is_file()
    {
        return Ok(Command::new(binary_path));
    }

    let binary_path = devo_binary_path()?;
    if binary_path.is_file() {
        return Ok(Command::new(binary_path));
    }

    let cargo_path = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"));
    let mut command = Command::new(cargo_path);
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("run")
        .arg("--quiet")
        .arg("-p")
        .arg("devo-cli")
        .arg("--bin")
        .arg("devo")
        .arg("--");
    Ok(command)
}

fn devo_binary_path() -> Result<PathBuf> {
    let mut path = std::env::current_exe()?;
    path.pop();
    path.pop();
    path.push(if cfg!(windows) { "devo.exe" } else { "devo" });
    Ok(path)
}

async fn read_websocket_json(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<serde_json::Value> {
    timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await.context("websocket closed")?? {
                Message::Text(text) => {
                    return serde_json::from_str(text.as_str()).map_err(Into::into);
                }
                _ => continue,
            }
        }
    })
    .await
    .context("timed out waiting for websocket message")?
}

async fn read_n_websocket_json(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    count: usize,
) -> Result<Vec<serde_json::Value>> {
    let mut values = Vec::with_capacity(count);
    while values.len() < count {
        values.push(read_websocket_json(socket).await?);
    }
    Ok(values)
}

async fn read_until_websocket_json<F>(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    predicate: F,
    max_messages: usize,
) -> Result<Vec<serde_json::Value>>
where
    F: Fn(&[serde_json::Value]) -> bool,
{
    let mut values = Vec::new();
    while values.len() < max_messages {
        values.push(read_websocket_json(socket).await?);
        if predicate(&values) {
            return Ok(values);
        }
    }
    anyhow::bail!("did not observe expected websocket messages within {max_messages} frames")
}

async fn parse_stdio_json_line(
    child: &mut tokio::process::Child,
    stderr_reader: &mut AsyncBufReader<tokio::process::ChildStderr>,
    context: &str,
    line: &str,
) -> Result<serde_json::Value> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        let mut stderr_output = String::new();
        stderr_reader.read_to_string(&mut stderr_output).await?;
        let exit_status = child.try_wait()?;
        anyhow::bail!(
            "{context} was empty; child_exit_status={exit_status:?}; child_stderr={stderr_output:?}"
        );
    }

    serde_json::from_str(trimmed).with_context(|| {
        let stderr_output = String::new();
        let _ = stderr_output;
        let exit_status = child.try_wait().ok().flatten();
        format!(
            "{context} was not valid JSON; raw_stdout_line={trimmed:?}; child_exit_status={exit_status:?}"
        )
    })
}

async fn read_stdio_line(
    reader: &mut tokio::io::Lines<AsyncBufReader<tokio::process::ChildStdout>>,
    context: &str,
) -> Result<String> {
    timeout(STDIO_SERVER_LINE_TIMEOUT, reader.next_line())
        .await
        .with_context(|| format!("timed out waiting for {context}"))?
        .with_context(|| format!("failed reading {context} from child stdout"))?
        .with_context(|| format!("{context} reached EOF before a line was produced"))
}
