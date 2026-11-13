//! Real subprocess/Native RPC/HTTP planner/global disk/new-session model path.
//! All homes, session artifacts, and provider data are synthetic and isolated.
#[path = "support/acp_session_setup.rs"]
mod support;

use anyhow::{Context, Result};
use devo_harness::HarnessState;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

async fn fixture() -> Result<(String, mpsc::Receiver<Value>, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (tx, rx) = mpsc::channel(32);
    let task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buf = [0; 4096];
                let request = loop {
                    let count = socket.read(&mut buf).await.unwrap();
                    assert!(count > 0, "fixture HTTP request closed early");
                    bytes.extend_from_slice(&buf[..count]);
                    assert!(bytes.len() < 2_000_000);
                    let Some(end) = bytes
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|n| n + 4)
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= end + length {
                        break serde_json::from_slice::<Value>(&bytes[end..end + length]).unwrap();
                    }
                };
                tx.send(request.clone()).await.unwrap();
                let (kind, body) = if request["stream"] == true {
                    let chunk = json!({"id":"fixture","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Completed isolated probe."},"finish_reason":null}]});
                    let done = json!({"id":"fixture","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
                    (
                        "text/event-stream",
                        format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n"),
                    )
                } else {
                    let candidate = request
                        .to_string()
                        .contains("Session memory candidates (JSON data)");
                    let content = if candidate {
                        json!({"summary":"Distill a reusable workflow","edits":[{"op":"create","kind":"memory","id":"model_chosen_id","after":{"title":"Narrow regression workflow","content":"Run a narrow regression before the complete suite.","source_id":"workflow","applicability":"When changing Devo code"}}]}).to_string()
                    } else {
                        json!({"summary":"No edits justified","edits":[]}).to_string()
                    };
                    ("application/json", json!({"id":"fixture","object":"chat.completion","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":content}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}).to_string())
                };
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            });
        }
    });
    Ok((format!("http://{address}"), rx, task))
}

async fn request(
    process: &mut support::StdioAcpProcess,
    id: i64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let reply = support::stdio_request_until(process, id, method, params, method).await?;
    anyhow::ensure!(reply["error"].is_null(), "{method}: {reply}");
    Ok(reply["result"].clone())
}

fn refinement_outcome(value: &Value) -> bool {
    value["method"] == "item/completed"
        && (value["params"]["item"]["item"]["type"] == "refinement"
            || value["params"]["item"]["item"]["code"] == "refinement_not_applied")
}

async fn wait_refine(process: &mut support::StdioAcpProcess) -> Result<Value> {
    support::read_stdio_json_until(
        &mut process.child,
        &mut process.stdout_reader,
        &mut process.stderr_reader,
        "refinement outcome",
        refinement_outcome,
    )
    .await
}

#[tokio::test]
async fn explicit_global_refine_distills_session_memory_and_next_session_consumes_it() -> Result<()>
{
    let temp = tempfile::TempDir::new()?;
    let (url, mut captures, fixture_task) = fixture().await?;
    support::write_test_config_with_extra(
        &temp,
        &["stdio://"],
        &url,
        "\n[tools]\nexecution_surface = \"discrete\"\n",
    )?;
    let home = temp.path().join(".devo");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace)?;
    let mut command = support::devo_command().await?;
    let mut child = command
        .arg("server")
        .arg("--transport")
        .arg("stdio")
        .env("DEVO_HOME", &home)
        .env("OPENAI_API_KEY", "isolated-fixture-key")
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut process = support::StdioAcpProcess {
        stdin: child.stdin.take().context("stdin")?,
        stdout_reader: BufReader::new(child.stdout.take().context("stdout")?).lines(),
        stderr_reader: BufReader::new(child.stderr.take().context("stderr")?),
        child,
    };
    request(
        &mut process,
        1,
        "initialize",
        json!({"protocolVersion":1,"clientCapabilities":{},"_meta":{"devo":{"protocol":"native"}}}),
    )
    .await?;
    let a = request(
        &mut process,
        2,
        "session/new",
        json!({"cwd":workspace,"idempotencyKey":"source-a"}),
    )
    .await?;
    let a_id = a["session"]["id"]
        .as_str()
        .context("session A id")?
        .to_string();
    let source_path = HarnessState::file_path(&home.join("session-artifacts").join(&a_id));
    let mut local = HarnessState::default();
    local.entries.get_mut("memory").unwrap().insert("workflow".into(),serde_json::from_value(json!({"id":"workflow","kind":"memory","title":"Repeated narrow regression success","content":"A narrow regression exposed the failure before the complete suite ran.","path":"Devo","scope":"local","source":"test","created_at":"2026-11-13T00:00:00Z","updated_at":"2026-11-13T00:00:00Z","version":1}))?);
    local.save_atomic(&source_path)?;
    let original = std::fs::read(&source_path)?;
    let global_path = HarnessState::file_path(&home);
    let mut global = HarnessState::default();
    let mut keep = local.entries["memory"]["workflow"].clone();
    keep.id = "existing".into();
    keep.content = "Keep existing global evidence.".into();
    keep.scope = Some(devo_harness::HarnessScope::Global);
    global
        .entries
        .get_mut("memory")
        .unwrap()
        .insert(keep.id.clone(), keep.clone());
    global.save_atomic(&global_path)?;
    // session/new subscribes this Native connection to its completion events.
    let admission_messages = support::stdio_collect_until(
        &mut process,
        4,
        "session/refine/run",
        json!({"sessionId":a_id,"global":true}),
        "global refine admission",
    )
    .await?;
    let admitted = admission_messages.last().context("admission response")?;
    assert_eq!(admitted["result"]["scheduled"], json!(true));
    let outcome = match admission_messages
        .iter()
        .find(|value| refinement_outcome(value))
    {
        Some(value) => value.clone(),
        None => wait_refine(&mut process).await?,
    };
    assert_eq!(
        outcome["params"]["item"]["item"]["type"],
        json!("refinement")
    );
    let stored = HarnessState::load(&global_path)?;
    assert_eq!(stored.entries["memory"].len(), 2);
    assert_eq!(stored.entries["memory"]["existing"], keep);
    assert_eq!(std::fs::read(&source_path)?, original);
    let plan_request = tokio::time::timeout(Duration::from_secs(30), async {
        let mut isolated_global_pass = false;
        loop {
            let value = captures.recv().await.context("provider capture closed")?;
            if value
                .to_string()
                .contains("Session memory candidates (JSON data)")
            {
                assert!(
                    isolated_global_pass,
                    "global consolidation must run independently first"
                );
                break Ok::<_, anyhow::Error>(value);
            }
            if value
                .to_string()
                .contains("You plan Continual Harness refinements")
            {
                assert!(
                    !value
                        .to_string()
                        .contains("A narrow regression exposed the failure"),
                    "global editor received local evidence"
                );
                isolated_global_pass = true;
            }
        }
    })
    .await??;
    assert!(
        plan_request
            .to_string()
            .contains("Repeated narrow regression success")
    );
    let b = request(
        &mut process,
        5,
        "session/new",
        json!({"cwd":workspace,"idempotencyKey":"consumer-b"}),
    )
    .await?;
    let b_id = b["session"]["id"].as_str().context("session B id")?;
    request(&mut process,6,"turn/start",json!({"sessionId":b_id,"input":[{"type":"text","text":"isolated-consumer-probe"}],"idempotencyKey":"consumer-probe"})).await?;
    let consumer = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let value = captures.recv().await.context("provider capture closed")?;
            if value["stream"] == true && value.to_string().contains("isolated-consumer-probe") {
                break Ok::<_, anyhow::Error>(value);
            }
        }
    })
    .await??;
    let text = consumer.to_string();
    assert!(text.contains("Run a narrow regression before the complete suite."));
    assert!(text.contains("Project: Devo"));
    assert!(text.contains("Keep existing global evidence."));
    assert!(
        !text.contains("A narrow regression exposed the failure"),
        "session A local text leaked to B"
    );
    support::shutdown_stdio_acp_process(process).await;
    fixture_task.abort();
    Ok(())
}
