use std::io;

use devo_protocol::ModelRequest;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_provider::ModelProviderSDK;
use devo_provider::ProviderHttpOptions;
use devo_provider::anthropic::AnthropicProvider;
use devo_provider::openai::OpenAIProvider;
use devo_provider::openai::OpenAIResponsesProvider;
use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// Trace: L2-DES-APP-005, L2-DES-MODEL-001
/// Verifies: OpenAI Chat Completions requests include custom provider headers and custom values override built-in headers.
#[tokio::test]
async fn openai_chat_completions_applies_custom_headers_after_builtin_headers() {
    let (base_url, capture) = spawn_json_server(openai_chat_response()).await;
    let provider = OpenAIProvider::new(base_url)
        .with_http_options(
            ProviderHttpOptions::from_raw(
                None,
                Some(
                    r#"{"Authorization":"custom-auth","Content-Type":"application/custom","X-Devo":"yes"}"#
                        .to_string(),
                ),
            )
            .expect("provider HTTP options"),
        )
        .expect("apply provider HTTP options")
        .with_api_key("builtin-key");

    provider
        .completion(minimal_request())
        .await
        .expect("provider response");
    let request = capture.await.expect("capture request");

    assert_eq!(header_value(&request, "authorization"), Some("custom-auth"));
    assert_eq!(
        header_value(&request, "content-type"),
        Some("application/custom")
    );
    assert_eq!(header_value(&request, "x-devo"), Some("yes"));
}

/// Trace: L2-DES-APP-005, L2-DES-MODEL-001
/// Verifies: OpenAI Responses requests include custom provider headers and custom values override built-in headers.
#[tokio::test]
async fn openai_responses_applies_custom_headers_after_builtin_headers() {
    let (base_url, capture) = spawn_json_server(openai_responses_response()).await;
    let provider = OpenAIResponsesProvider::new(base_url)
        .with_http_options(
            ProviderHttpOptions::from_raw(
                None,
                Some(
                    r#"{"Authorization":"custom-auth","Content-Type":"application/custom","X-Devo":"yes"}"#
                        .to_string(),
                ),
            )
            .expect("provider HTTP options"),
        )
        .expect("apply provider HTTP options")
        .with_api_key("builtin-key");

    provider
        .completion(minimal_request())
        .await
        .expect("provider response");
    let request = capture.await.expect("capture request");

    assert_eq!(header_value(&request, "authorization"), Some("custom-auth"));
    assert_eq!(
        header_value(&request, "content-type"),
        Some("application/custom")
    );
    assert_eq!(header_value(&request, "x-devo"), Some("yes"));
}

/// Trace: L2-DES-APP-005, L2-DES-MODEL-001
/// Verifies: Anthropic Messages requests include custom provider headers and custom values override built-in headers.
#[tokio::test]
async fn anthropic_messages_applies_custom_headers_after_builtin_headers() {
    let (base_url, capture) = spawn_json_server(anthropic_response()).await;
    let provider = AnthropicProvider::new(base_url)
        .with_http_options(
            ProviderHttpOptions::from_raw(
                None,
                Some(
                    r#"{"x-api-key":"custom-key","anthropic-version":"custom-version","Content-Type":"application/custom","X-Devo":"yes"}"#
                        .to_string(),
                ),
            )
            .expect("provider HTTP options"),
        )
        .expect("apply provider HTTP options")
        .with_api_key("builtin-key");

    provider
        .completion(minimal_request())
        .await
        .expect("provider response");
    let request = capture.await.expect("capture request");

    assert_eq!(header_value(&request, "x-api-key"), Some("custom-key"));
    assert_eq!(
        header_value(&request, "anthropic-version"),
        Some("custom-version")
    );
    assert_eq!(
        header_value(&request, "content-type"),
        Some("application/custom")
    );
    assert_eq!(header_value(&request, "x-devo"), Some("yes"));
}

/// Trace: L2-DES-APP-005, L2-DES-MODEL-001
/// Verifies: provider proxy configuration routes OpenAI Chat Completions requests through the configured proxy.
#[tokio::test]
async fn provider_proxy_routes_openai_chat_completions_request() {
    let (proxy_url, capture) = spawn_json_server(openai_chat_response()).await;
    let provider = OpenAIProvider::new("http://provider.example/v1")
        .with_http_options(
            ProviderHttpOptions::from_raw(Some(proxy_url), None).expect("provider HTTP options"),
        )
        .expect("apply provider HTTP options");

    provider
        .completion(minimal_request())
        .await
        .expect("provider response");
    let request = capture.await.expect("capture request");

    assert_eq!(
        request.lines().next(),
        Some("POST http://provider.example/v1/chat/completions HTTP/1.1")
    );
}

async fn spawn_json_server(
    response_body: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept request");
        let request = read_http_headers(&mut socket).await.expect("read request");
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write response");
        request
    });

    (format!("http://{addr}"), handle)
}

async fn read_http_headers(socket: &mut tokio::net::TcpStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let count = socket.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn header_value<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.lines().skip(1).find_map(|line| {
        let (header_name, value) = line.split_once(':')?;
        header_name.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn minimal_request() -> ModelRequest {
    ModelRequest {
        model: "test-model".to_string(),
        system: None,
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: vec![RequestContent::Text {
                text: "Reply with OK only.".to_string(),
            }],
        }],
        max_tokens: 16,
        tools: None,
        sampling: Default::default(),
        thinking: None,
        reasoning_effort: None,
        extra_body: None,
    }
}

fn openai_chat_response() -> &'static str {
    r#"{"id":"chatcmpl-test","object":"chat.completion","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"OK"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#
}

fn openai_responses_response() -> &'static str {
    r#"{"id":"resp-test","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"OK"}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#
}

fn anthropic_response() -> &'static str {
    r#"{"id":"msg-test","type":"message","role":"assistant","model":"claude-test","content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#
}
