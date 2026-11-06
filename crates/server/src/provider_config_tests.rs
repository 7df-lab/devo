//! Regression tests for pinned, model-specific connection routes.

use crate::provider_config::{build_connection_route_table, load_server_provider};
use devo_core::{
    AppConfig, CatalogConfig, ProviderConfigFile, ProviderHttpConfig, UserAuthConfigFile,
};
use devo_protocol::{ModelRequest, ProviderWireApi};
use devo_provider::ProviderRoute;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn pinned_offline_models_use_distinct_endpoint_and_protocol_routes() {
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config: ProviderConfigFile = serde_json::from_value(json!({
        "model": "pinned/first", "provider": {"pinned": {
            "base_url": format!("http://{address}/default"),
            "wire_api": "openai_chat_completions", "credential": "pinned",
            "models": {
                "first": {"base_url": format!("http://{address}/first"), "wire_api": "openai_chat_completions"},
                "second": {"base_url": format!("http://{address}/second"), "wire_api": "openai_chat_completions"},
                "third": {"base_url": format!("http://{address}/third/v1"), "wire_api": "anthropic_messages"}
            }
        }}
    })).unwrap();
    let auth_json = json!({"pinned": {"kind": "api_key", "value": "fixture-key"}});
    std::fs::write(home.path().join("auth.json"), auth_json.to_string()).unwrap();
    let auth: UserAuthConfigFile =
        devo_core::read_user_auth_config(&home.path().join("auth.json")).unwrap();
    let http = ProviderHttpConfig {
        no_proxy: Some("127.0.0.1".to_string()),
        ..Default::default()
    };
    let routes = build_connection_route_table(&config, &config, &http, &auth, home.path())
        .await
        .unwrap();
    let keys = routes
        .iter()
        .map(|(route, _)| route.clone())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        keys,
        [
            ProviderRoute::connection("pinned", ProviderWireApi::OpenAIChatCompletions),
            ProviderRoute::connection("pinned", ProviderWireApi::AnthropicMessages),
            ProviderRoute::connection_model(
                "pinned",
                ProviderWireApi::OpenAIChatCompletions,
                "first"
            ),
            ProviderRoute::connection_model(
                "pinned",
                ProviderWireApi::OpenAIChatCompletions,
                "second"
            ),
            ProviderRoute::connection_model("pinned", ProviderWireApi::AnthropicMessages, "third"),
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(routes.len(), keys.len());

    let source = tokio::spawn(async move {
        let mut observed = Vec::new();
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0, "request ended before its body");
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= header_end + 4 + length {
                        let body: serde_json::Value =
                            serde_json::from_slice(&bytes[header_end + 4..header_end + 4 + length])
                                .unwrap();
                        observed.push((
                            headers.lines().next().unwrap().to_string(),
                            body["model"].clone(),
                        ));
                        break;
                    }
                }
            }
            // Both wire protocols receive a valid minimal success response.
            let body = json!({"id": "fixture", "type": "message", "role": "assistant", "model": "fixture",
                "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
                "usage": {"input_tokens": 1, "output_tokens": 1, "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }).to_string();
            let length = body.len();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}").as_bytes()).await.unwrap();
        }
        observed
    });
    let app = AppConfig {
        provider_catalog: config,
        catalog: CatalogConfig {
            offline: true,
            ..Default::default()
        },
        provider_http: http,
        ..Default::default()
    };
    let resolved = load_server_provider(&app, /*default_model*/ None, home.path())
        .await
        .unwrap();
    for (model, protocol) in [
        ("first", ProviderWireApi::OpenAIChatCompletions),
        ("second", ProviderWireApi::OpenAIChatCompletions),
        ("third", ProviderWireApi::AnthropicMessages),
    ] {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: model.to_string(),
            system: None,
            messages: Vec::new(),
            max_tokens: 1,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: Default::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };
        resolved
            .provider_router
            .complete(
                ProviderRoute::connection_model("pinned", protocol, model),
                request,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        source.await.unwrap(),
        vec![
            (
                "POST /first/chat/completions HTTP/1.1".to_string(),
                json!("first")
            ),
            (
                "POST /second/chat/completions HTTP/1.1".to_string(),
                json!("second")
            ),
            (
                "POST /third/v1/messages HTTP/1.1".to_string(),
                json!("third")
            ),
        ]
    );
    assert!(!home.path().join("cache/models.dev-api.json").exists());
}
