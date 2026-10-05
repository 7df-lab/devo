#[path = "support/acp_session_setup.rs"]
mod acp_session_setup;

use anyhow::Context;
use anyhow::Result;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use acp_session_setup::AcpStdioClientInfo;
use acp_session_setup::acp_config_option;
use acp_session_setup::acp_config_option_optional;
use acp_session_setup::acp_model_config_option;
use acp_session_setup::assert_config_option_lacks_value;
use acp_session_setup::assert_config_option_values;
use acp_session_setup::assert_no_history_replay_before_response;
use acp_session_setup::assert_replayed_history_before_response;
use acp_session_setup::assert_stdio_prompt_turn;
use acp_session_setup::build_test_mcp_server_binary;
use acp_session_setup::mcp_stdio_server_config;
use acp_session_setup::shutdown_stdio_acp_process;
use acp_session_setup::spawn_openai_chat_completions_server;
use acp_session_setup::spawn_stdio_acp_process;
use acp_session_setup::stdio_collect_until;
use acp_session_setup::stdio_initialize;
use acp_session_setup::stdio_request_until;
use acp_session_setup::write_acp_prompt;
use acp_session_setup::write_test_config;

#[tokio::test]
async fn stdio_acp_load_and_resume_keep_session_mcp_tools_hidden_from_models() -> Result<()> {
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
    let mut process = spawn_stdio_acp_process(home_dir.path()).await?;
    let client = AcpStdioClientInfo {
        name: "acp-session-setup-e2e",
        title: "ACP Session Setup E2E",
    };

    let initialize_response = stdio_initialize(&mut process, 0, client, json!({})).await?;
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["loadSession"],
        json!(true)
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"]["resume"],
        json!({})
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"]["close"],
        json!({})
    );
    assert_eq!(
        initialize_response["result"]["agentCapabilities"]["sessionCapabilities"]["additionalDirectories"],
        json!({})
    );

    let session_new_response = stdio_request_until(
        &mut process,
        1,
        "session/new",
        json!({
            "cwd": cwd,
            "additionalDirectories": [additional_directory],
            "mcpServers": []
        }),
        "ACP session/new response",
    )
    .await?;
    let session_id = session_new_response["result"]["sessionId"]
        .as_str()
        .context("session/new response included a sessionId")?
        .to_string();

    assert_stdio_prompt_turn(
        &mut process,
        &mut provider,
        2,
        &session_id,
        "create one replayable ACP history item",
        "initial session/prompt response",
        "initial provider prompt request",
        Some("ipython"),
        Some("mcp__load_tools__echo"),
    )
    .await?;

    let load_messages = stdio_collect_until(
        &mut process,
        3,
        "session/load",
        json!({
            "sessionId": session_id,
            "cwd": cwd,
            "additionalDirectories": [additional_directory],
            "mcpServers": [mcp_stdio_server_config("load-tools", &mcp_server_binary)?]
        }),
        "ACP session/load response",
    )
    .await?;
    let load_response = load_messages
        .last()
        .context("session/load produced a response")?;
    assert_eq!(load_response["id"], json!(3));
    let _ = acp_config_option(&load_response["result"], "model")?;
    assert_replayed_history_before_response(&load_messages, &session_id)?;

    assert_stdio_prompt_turn(
        &mut process,
        &mut provider,
        4,
        &session_id,
        "after load, keep session MCP tools hidden from the model",
        "post-load session/prompt response",
        "post-load provider prompt request",
        Some("ipython"),
        Some("mcp__load_tools__echo"),
    )
    .await?;

    let resume_messages = stdio_collect_until(
        &mut process,
        5,
        "session/resume",
        json!({
            "sessionId": session_id,
            "cwd": cwd,
            "additionalDirectories": [additional_directory],
            "mcpServers": [mcp_stdio_server_config("resume-tools", &mcp_server_binary)?]
        }),
        "ACP session/resume response",
    )
    .await?;
    let resume_response = resume_messages
        .last()
        .context("session/resume produced a response")?;
    assert_eq!(resume_response["id"], json!(5));
    assert!(resume_response["result"].is_object());
    assert_no_history_replay_before_response(&resume_messages)?;

    let post_resume_request = assert_stdio_prompt_turn(
        &mut process,
        &mut provider,
        6,
        &session_id,
        "after resume, keep session MCP tools hidden from the model",
        "post-resume session/prompt response",
        "post-resume provider prompt request",
        Some("ipython"),
        Some("mcp__resume_tools__echo"),
    )
    .await?;
    acp_session_setup::assert_openai_request_lacks_mcp_tool(
        &post_resume_request,
        "mcp__load_tools__echo",
    )?;

    assert_eq!(
        stdio_request_until(
            &mut process,
            7,
            "session/close",
            json!({ "sessionId": session_id }),
            "ACP session/close response",
        )
        .await?,
        json!({ "jsonrpc": "2.0", "id": 7, "result": {} })
    );
    shutdown_stdio_acp_process(process).await;
    Ok(())
}

#[tokio::test]
async fn stdio_acp_session_config_options_select_model_binding() -> Result<()> {
    let home_dir = TempDir::new()?;
    let mut provider = spawn_openai_chat_completions_server().await?;
    write_test_config(&home_dir, &["stdio://"], &provider.base_url)?;
    let cwd = home_dir.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    std::fs::create_dir_all(cwd.join(".devo"))?;
    std::fs::write(
        cwd.join(".devo").join("config.toml"),
        r#"
[model.test-model]
display_name = "Test Model"
reasoning_capability = { levels = ["low", "medium", "high"] }
default_reasoning_effort = "medium"
base_instructions = "Test model instructions"

[model.alt-model]
display_name = "Alt Model"
base_instructions = "Alt model instructions"

[model.catalog-only-model]
display_name = "Catalog Only Model"
base_instructions = "Catalog-only model instructions"
"#,
    )?;
    let cwd = cwd.to_string_lossy().into_owned();
    let mut process = spawn_stdio_acp_process(home_dir.path()).await?;
    stdio_initialize(
        &mut process,
        0,
        AcpStdioClientInfo {
            name: "acp-config-options-e2e",
            title: "ACP Config Options E2E",
        },
        json!({}),
    )
    .await?;

    let session_new_response = stdio_request_until(
        &mut process,
        1,
        "session/new",
        json!({ "cwd": cwd, "mcpServers": [] }),
        "ACP session/new response",
    )
    .await?;
    let session_id = session_new_response["result"]["sessionId"]
        .as_str()
        .context("session/new response included a sessionId")?
        .to_string();
    let model_option = acp_model_config_option(&session_new_response["result"])?;
    assert_eq!(model_option["name"], json!("Model"));
    assert_eq!(model_option["category"], json!("model"));
    assert_eq!(model_option["currentValue"], json!("openai/test-model"));
    assert_config_option_values(model_option, &["openai/alt-model", "openai/test-model"])?;
    assert_config_option_lacks_value(model_option, "catalog-only-model")?;
    let reasoning_effort_option =
        acp_config_option(&session_new_response["result"], "thought_level")?;
    assert_eq!(reasoning_effort_option["name"], json!("Reasoning Effort"));
    assert_eq!(reasoning_effort_option["category"], json!("thought_level"));
    assert_eq!(reasoning_effort_option["currentValue"], json!("medium"));
    assert_config_option_values(reasoning_effort_option, &["low", "medium", "high"])?;
    let mode_option = acp_config_option(&session_new_response["result"], "mode")?;
    assert_eq!(mode_option["name"], json!("Session Mode"));
    assert_eq!(mode_option["currentValue"], json!("auto-review"));
    assert_config_option_values(mode_option, &["default", "auto-review", "full-access"])?;

    let set_reasoning_effort_response = stdio_request_until(
        &mut process,
        2,
        "session/set_config_option",
        json!({
            "sessionId": session_id,
            "configId": "thought_level",
            "value": "high"
        }),
        "ACP session/set_config_option reasoning effort response",
    )
    .await?;
    let reasoning_effort_option =
        acp_config_option(&set_reasoning_effort_response["result"], "thought_level")?;
    assert_eq!(reasoning_effort_option["currentValue"], json!("high"));
    assert_eq!(
        acp_model_config_option(&set_reasoning_effort_response["result"])?["currentValue"],
        json!("openai/test-model")
    );

    let provider_request = assert_stdio_prompt_turn(
        &mut process,
        &mut provider,
        3,
        &session_id,
        "use the selected ACP reasoning effort",
        "ACP session/prompt response after reasoning effort update",
        "provider prompt request after reasoning effort option update",
        None,
        None,
    )
    .await?;
    assert_eq!(provider_request["model"], json!("test-model"));
    assert_eq!(provider_request["reasoning_effort"], json!("high"));

    let set_config_response = stdio_request_until(
        &mut process,
        4,
        "session/set_config_option",
        json!({
            "sessionId": session_id,
            "configId": "model",
            "value": "openai/alt-model"
        }),
        "ACP session/set_config_option response",
    )
    .await?;
    assert_eq!(
        acp_model_config_option(&set_config_response["result"])?["currentValue"],
        json!("openai/alt-model")
    );
    assert!(acp_config_option_optional(&set_config_response["result"], "thought_level").is_none());
    assert_eq!(
        acp_config_option(&set_config_response["result"], "mode")?["currentValue"],
        json!("auto-review")
    );

    let set_mode_response = stdio_request_until(
        &mut process,
        5,
        "session/set_config_option",
        json!({
            "sessionId": session_id,
            "configId": "mode",
            "value": "full-access"
        }),
        "ACP session/set_config_option mode response",
    )
    .await?;
    assert_eq!(
        acp_config_option(&set_mode_response["result"], "mode")?["currentValue"],
        json!("full-access")
    );
    assert_eq!(
        acp_model_config_option(&set_mode_response["result"])?["currentValue"],
        json!("openai/alt-model")
    );

    let provider_request = assert_stdio_prompt_turn(
        &mut process,
        &mut provider,
        6,
        &session_id,
        "use the selected ACP model binding",
        "ACP session/prompt response",
        "provider prompt request after config option update",
        None,
        None,
    )
    .await?;
    assert_eq!(provider_request["model"], json!("alt-model"));
    // Hosted web_search is Anthropic Messages / OpenAI Responses only; the
    // chat completions wire stays local-only and must not receive
    // `web_search_options`.
    assert_eq!(provider_request.get("web_search_options"), None);

    shutdown_stdio_acp_process(process).await;
    Ok(())
}

#[tokio::test]
async fn stdio_proxy_acp_prompt_streams_each_agent_chunk_once() -> Result<()> {
    let home_dir = TempDir::new()?;
    let mut provider = spawn_openai_chat_completions_server().await?;
    write_test_config(&home_dir, &["stdio://"], &provider.base_url)?;
    let cwd = home_dir.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    let cwd = cwd.to_string_lossy().into_owned();

    let mut first = spawn_stdio_acp_process(home_dir.path()).await?;
    stdio_initialize(
        &mut first,
        0,
        AcpStdioClientInfo {
            name: "acp-real-server-holder",
            title: "ACP Real Server Holder",
        },
        json!({}),
    )
    .await?;
    let first_session_new_response = stdio_request_until(
        &mut first,
        1,
        "session/new",
        json!({ "cwd": cwd, "mcpServers": [] }),
        "real server session/new response",
    )
    .await?;
    let session_id = first_session_new_response["result"]["sessionId"]
        .as_str()
        .context("real server session/new response included a sessionId")?
        .to_string();
    assert_stdio_prompt_turn(
        &mut first,
        &mut provider,
        2,
        &session_id,
        "create history before proxy load",
        "real server session/prompt response",
        "real server provider prompt request",
        None,
        None,
    )
    .await?;

    let mut proxy = spawn_stdio_acp_process(home_dir.path()).await?;
    stdio_initialize(
        &mut proxy,
        3,
        AcpStdioClientInfo {
            name: "third-party-acp-proxy-client",
            title: "Third Party ACP Proxy Client",
        },
        json!({}),
    )
    .await?;
    let session_load_messages = stdio_collect_until(
        &mut proxy,
        4,
        "session/load",
        json!({
            "sessionId": session_id,
            "cwd": cwd,
            "additionalDirectories": [],
            "mcpServers": []
        }),
        "proxy session/load response",
    )
    .await?;
    let session_load_response = session_load_messages
        .last()
        .context("proxy session/load produced a response")?;
    assert_eq!(session_load_response["id"], json!(4));
    let _ = acp_config_option(&session_load_response["result"], "model")?;
    assert_replayed_history_before_response(&session_load_messages, &session_id)?;

    write_acp_prompt(
        &mut proxy.stdin,
        5,
        &session_id,
        "stream one ACP proxy reply",
    )
    .await?;
    let prompt_messages = acp_session_setup::read_stdio_json_collect_until(
        &mut proxy.child,
        &mut proxy.stdout_reader,
        &mut proxy.stderr_reader,
        "proxy session/prompt response",
        |value| value.get("id") == Some(&json!(5)),
    )
    .await?;
    let prompt_response = prompt_messages
        .last()
        .context("proxy session/prompt produced a response")?;
    acp_session_setup::assert_prompt_response(prompt_response, 5);
    let chunks = prompt_messages
        .iter()
        .filter_map(|message| {
            if message["method"] != json!("session/update")
                || message["params"]["sessionId"].as_str() != Some(session_id.as_str())
                || message["params"]["update"]["sessionUpdate"].as_str()
                    != Some("agent_message_chunk")
            {
                return None;
            }
            message["params"]["update"]["content"]["text"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    assert_eq!(chunks, vec!["ACP compatibility response.".to_string()]);
    let _ = acp_session_setup::recv_provider_prompt_request(
        &mut provider.requests,
        "proxy provider prompt request",
        "stream one ACP proxy reply",
    )
    .await?;

    shutdown_stdio_acp_process(proxy).await;
    shutdown_stdio_acp_process(first).await;
    Ok(())
}
