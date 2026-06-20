#[path = "support/acp_session_setup.rs"]
mod acp_session_setup;

use anyhow::Context;
use anyhow::Result;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use acp_session_setup::STDIO_SERVER_STARTUP_TIMEOUT;
use acp_session_setup::assert_no_history_replay_before_response;
use acp_session_setup::assert_openai_request_has_mcp_tool;
use acp_session_setup::assert_openai_request_lacks_mcp_tool;
use acp_session_setup::assert_prompt_response;
use acp_session_setup::assert_replayed_history_before_response;
use acp_session_setup::build_test_mcp_server_binary;
use acp_session_setup::devo_command;
use acp_session_setup::mcp_stdio_server_config;
use acp_session_setup::read_stdio_json;
use acp_session_setup::read_stdio_json_collect_until;
use acp_session_setup::read_stdio_json_until;
use acp_session_setup::recv_provider_prompt_request;
use acp_session_setup::spawn_openai_chat_completions_server;
use acp_session_setup::write_acp_prompt;
use acp_session_setup::write_stdio_json;
use acp_session_setup::write_test_config;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader as AsyncBufReader;

#[tokio::test]
async fn stdio_acp_load_and_resume_match_session_setup_contract() -> Result<()> {
    let home_dir = TempDir::new()?;
    let mut provider = spawn_openai_chat_completions_server().await?;
    write_test_config(&home_dir, &["stdio://"], &provider.base_url)?;

    let cwd = home_dir.path().join("workspace");
    let additional_directory = home_dir.path().join("shared");
    std::fs::create_dir_all(&cwd)?;
    std::fs::create_dir_all(&additional_directory)?;
    let cwd = cwd.to_string_lossy().into_owned();
    let additional_directory = additional_directory.to_string_lossy().into_owned();
    let mcp_server_binary = build_test_mcp_server_binary().await?;

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

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "acp-session-setup-e2e",
                    "title": "ACP Session Setup E2E",
                    "version": "1.0.0"
                }
            }
        }),
    )
    .await?;

    let initialize_response = read_stdio_json(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP initialize response",
        STDIO_SERVER_STARTUP_TIMEOUT,
    )
    .await?;
    assert_eq!(initialize_response["jsonrpc"], serde_json::json!("2.0"));
    assert_eq!(initialize_response["id"], serde_json::json!(0));
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["loadSession"],
        serde_json::json!(true)
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"]["resume"],
        serde_json::json!({})
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"]["close"],
        serde_json::json!({})
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"]["additionalDirectories"],
        serde_json::json!({})
    );

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session/new",
            "params": {
                "cwd": cwd,
                "additionalDirectories": [additional_directory],
                "mcpServers": []
            }
        }),
    )
    .await?;
    let session_new_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/new response",
        |value| value.get("id") == Some(&serde_json::json!(1)),
    )
    .await?;
    let session_id = session_new_response["result"]["sessionId"]
        .as_str()
        .context("session/new response included a sessionId")?
        .to_string();

    let initial_prompt = "create one replayable ACP history item";
    write_acp_prompt(&mut stdin, 2, &session_id, initial_prompt).await?;
    let initial_prompt_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "initial session/prompt response",
        |value| value.get("id") == Some(&serde_json::json!(2)),
    )
    .await?;
    assert_prompt_response(&initial_prompt_response, 2);
    let initial_provider_request = recv_provider_prompt_request(
        &mut provider.requests,
        "initial provider prompt request",
        initial_prompt,
    )
    .await?;
    assert_openai_request_lacks_mcp_tool(&initial_provider_request, "mcp__load_tools__echo")?;

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/load",
            "params": {
                "sessionId": session_id,
                "cwd": cwd,
                "additionalDirectories": [additional_directory],
                "mcpServers": [mcp_stdio_server_config("load-tools", &mcp_server_binary)?]
            }
        }),
    )
    .await?;
    let load_messages = read_stdio_json_collect_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/load response",
        |value| value.get("id") == Some(&serde_json::json!(3)),
    )
    .await?;
    let load_response = load_messages
        .last()
        .context("session/load produced a response")?;
    assert_eq!(
        load_response,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "result": null
        })
    );
    assert_replayed_history_before_response(&load_messages, &session_id)?;

    let load_prompt = "after load, declare load MCP tools";
    write_acp_prompt(&mut stdin, 4, &session_id, load_prompt).await?;
    let load_prompt_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "post-load session/prompt response",
        |value| value.get("id") == Some(&serde_json::json!(4)),
    )
    .await?;
    assert_prompt_response(&load_prompt_response, 4);
    let load_provider_request = recv_provider_prompt_request(
        &mut provider.requests,
        "post-load provider prompt request",
        load_prompt,
    )
    .await?;
    assert_openai_request_has_mcp_tool(&load_provider_request, "mcp__load_tools__echo")?;

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "session/resume",
            "params": {
                "sessionId": session_id,
                "cwd": cwd,
                "additionalDirectories": [additional_directory],
                "mcpServers": [mcp_stdio_server_config("resume-tools", &mcp_server_binary)?]
            }
        }),
    )
    .await?;
    let resume_messages = read_stdio_json_collect_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/resume response",
        |value| value.get("id") == Some(&serde_json::json!(5)),
    )
    .await?;
    let resume_response = resume_messages
        .last()
        .context("session/resume produced a response")?;
    assert_eq!(resume_response["jsonrpc"], serde_json::json!("2.0"));
    assert_eq!(resume_response["id"], serde_json::json!(5));
    assert!(resume_response["result"].is_object());
    assert_no_history_replay_before_response(&resume_messages)?;

    let resume_prompt = "after resume, declare resume MCP tools";
    write_acp_prompt(&mut stdin, 6, &session_id, resume_prompt).await?;
    let resume_prompt_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "post-resume session/prompt response",
        |value| value.get("id") == Some(&serde_json::json!(6)),
    )
    .await?;
    assert_prompt_response(&resume_prompt_response, 6);
    let resume_provider_request = recv_provider_prompt_request(
        &mut provider.requests,
        "post-resume provider prompt request",
        resume_prompt,
    )
    .await?;
    assert_openai_request_has_mcp_tool(&resume_provider_request, "mcp__resume_tools__echo")?;
    assert_openai_request_lacks_mcp_tool(&resume_provider_request, "mcp__load_tools__echo")?;

    write_stdio_json(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "session/close",
            "params": {
                "sessionId": session_id
            }
        }),
    )
    .await?;
    let close_response = read_stdio_json_until(
        &mut child,
        &mut stdout_reader,
        &mut stderr_reader,
        "ACP session/close response",
        |value| value.get("id") == Some(&serde_json::json!(7)),
    )
    .await?;
    assert_eq!(
        close_response,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": {}
        })
    );

    drop(stdin);
    child.kill().await.ok();
    let _ = child.wait().await;
    Ok(())
}
