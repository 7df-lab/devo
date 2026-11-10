//! Verify normalization through the real HTTP/SSE provider adapters.

use std::sync::{Arc, Mutex};

use devo_protocol::{ModelProfileKey, ModelRequest, StreamEvent, Usage};
use devo_provider::ModelProviderSDK;
use devo_provider::anthropic::AnthropicProvider;
use devo_provider::google::GoogleGenerativeAiProvider;
use devo_provider::openai::{OpenAIProvider, OpenAIResponsesProvider};
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Copy, Debug)]
enum Family {
    Chat,
    Responses,
    Anthropic,
    Google,
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Completion,
    Stream,
}

impl Family {
    fn provider(self, url: String) -> Box<dyn ModelProviderSDK> {
        match self {
            Self::Chat => Box::new(OpenAIProvider::new(url)),
            Self::Responses => Box::new(OpenAIResponsesProvider::new(url)),
            Self::Anthropic => Box::new(AnthropicProvider::new(url)),
            Self::Google => Box::new(GoogleGenerativeAiProvider::new(url, "test-key")),
        }
    }

    fn zero(self) -> Value {
        match self {
            Self::Chat => json!({"prompt_tokens": 0, "completion_tokens": 0}),
            Self::Responses | Self::Anthropic => json!({"input_tokens": 0, "output_tokens": 0}),
            Self::Google => json!({"promptTokenCount": 0, "candidatesTokenCount": 0}),
        }
    }

    fn response(self, usage: Option<Value>) -> Value {
        let mut response = match self {
            Self::Chat => json!({"id": "chat", "choices": []}),
            Self::Responses => json!({"id": "response", "status": "completed", "output": []}),
            Self::Anthropic => json!({"id": "message", "content": [], "stop_reason": "end_turn"}),
            Self::Google => json!({"responseId": "google", "candidates": []}),
        };
        if let Some(usage) = usage {
            let field = match self {
                Self::Google => "usageMetadata",
                Self::Chat | Self::Responses | Self::Anthropic => "usage",
            };
            response[field] = usage;
        }
        response
    }

    fn body(self, mode: Mode, usage: Option<Value>) -> (String, &'static str) {
        let response = self.response(usage);
        match (self, mode) {
            (Self::Google, _) | (Self::Chat | Self::Anthropic, Mode::Completion) => {
                (response.to_string(), "application/json")
            }
            (Self::Chat, Mode::Stream) => (
                format!("data: {response}\n\ndata: [DONE]\n\n"),
                "text/event-stream",
            ),
            (Self::Responses, _) => (
                format!(
                    "event: response.completed\ndata: {}\n\n",
                    json!({"response": response})
                ),
                "text/event-stream",
            ),
            (Self::Anthropic, Mode::Stream) => (
                format!(
                    "event: message_start\ndata: {}\n\nevent: message_stop\ndata: {{}}\n\n",
                    json!({"message": response})
                ),
                "text/event-stream",
            ),
        }
    }
}

#[tokio::test]
async fn all_providers_distinguish_missing_placeholders_and_reported_zero() {
    for family in [
        Family::Chat,
        Family::Responses,
        Family::Anthropic,
        Family::Google,
    ] {
        for mode in [Mode::Completion, Mode::Stream] {
            for usage in [
                None,
                Some(Value::Null),
                Some(json!({})),
                Some(json!({"service_tier": "default"})),
                Some(family.zero()),
            ] {
                let expected = Usage {
                    reported: usage.as_ref() == Some(&family.zero()),
                    ..Usage::default()
                }
                .with_field_coverage(
                    /*input_tokens*/ Some(0),
                    /*output_tokens*/ Some(0),
                );
                let (body, content_type) = family.body(mode, usage.clone());
                let (url, server) = serve(body, content_type).await;
                let provider = family.provider(url);
                match mode {
                    Mode::Completion => {
                        let response = provider.completion(request()).await.expect("completion");
                        assert_eq!(response.usage, expected, "{family:?} {mode:?}: {usage:?}");
                    }
                    Mode::Stream => {
                        let mut stream =
                            provider.completion_stream(request()).await.expect("stream");
                        let mut samples = Vec::new();
                        let mut done = None;
                        while let Some(event) = stream.next().await {
                            match event.expect("stream event") {
                                StreamEvent::UsageDelta(usage) => samples.push(usage),
                                StreamEvent::MessageDone { response } => {
                                    done = Some(response.usage)
                                }
                                _ => {}
                            }
                        }
                        assert_eq!(done, Some(expected.clone()), "{family:?}: {usage:?}");
                        // Responses exposes its terminal sample in MessageDone;
                        // other adapters also emit the normalized UsageDelta.
                        let expected_samples =
                            if expected.is_reported() && !matches!(family, Family::Responses) {
                                vec![expected]
                            } else {
                                Vec::new()
                            };
                        assert_eq!(samples, expected_samples, "{family:?}: {usage:?}");
                    }
                }
                server.await.expect("server task");
            }
        }
    }
}

#[tokio::test]
async fn streaming_placeholders_do_not_erase_a_reported_sample() {
    for family in [Family::Chat, Family::Responses, Family::Anthropic] {
        let reported = family.zero();
        let body = match family {
            Family::Chat => format!(
                "data: {}\n\ndata: {{\"usage\":{{}}}}\n\ndata: [DONE]\n\n",
                json!({"id": "chat", "usage": reported})
            ),
            Family::Responses => format!(
                "data: {}\n\nevent: response.completed\ndata: {}\n\n",
                json!({"usage": reported}),
                json!({"response": {"status": "completed", "usage": {}}})
            ),
            Family::Anthropic => format!(
                "event: message_start\ndata: {}\n\nevent: message_delta\ndata: {{\"usage\":{{}}}}\n\nevent: message_stop\ndata: {{}}\n\n",
                json!({"message": {"id": "message", "usage": reported}})
            ),
            Family::Google => unreachable!("Google uses a synthetic stream"),
        };
        let (url, server) = serve(body, "text/event-stream").await;
        let provider = family.provider(url);
        let mut stream = provider.completion_stream(request()).await.expect("stream");
        let mut usage_events = Vec::new();
        while let Some(event) = stream.next().await {
            match event.expect("stream event") {
                StreamEvent::UsageDelta(usage) => usage_events.push(usage),
                StreamEvent::MessageDone { response } => usage_events.push(response.usage),
                _ => {}
            }
        }
        let expected = Usage {
            reported: true,
            ..Usage::default()
        }
        .with_field_coverage(
            /*input_tokens*/ Some(0),
            /*output_tokens*/ Some(0),
        );
        assert_eq!(usage_events, vec![expected.clone(), expected], "{family:?}");
        server.await.expect("server task");
    }
}

#[tokio::test]
async fn google_preserves_reported_totals_cache_and_thought_tokens() {
    let usage = json!({
        "promptTokenCount": 10, "candidatesTokenCount": 3,
        "thoughtsTokenCount": 5, "cachedContentTokenCount": 2,
        "totalTokenCount": 18
    });
    let (body, content_type) = Family::Google.body(Mode::Completion, Some(usage));
    let (url, server) = serve(body, content_type).await;
    let response = Family::Google
        .provider(url)
        .completion(request())
        .await
        .expect("completion");
    assert_eq!(
        response.usage,
        Usage {
            reported: true,
            input_tokens: 10,
            output_tokens: 3,
            reasoning_output_tokens: Some(5),
            cache_read_input_tokens: Some(2),
            total_tokens: Some(18),
            ..Usage::default()
        }
        .with_field_coverage(
            /*input_tokens*/ Some(0),
            /*output_tokens*/ Some(0)
        )
    );
    server.await.expect("server task");
}

#[tokio::test]
async fn failed_responses_emit_reported_usage_before_the_error() {
    let failure = json!({"response": {
        "status": "failed", "usage": {"input_tokens": 7, "output_tokens": 2},
        "error": {"code": "server_error", "message": "failed after generation"}
    }});
    let (url, server) = serve(
        format!("event: response.failed\ndata: {failure}\n\n"),
        "text/event-stream",
    )
    .await;
    let provider = Family::Responses.provider(url);
    let mut stream = provider.completion_stream(request()).await.expect("stream");
    assert_eq!(
        stream.next().await.expect("usage").expect("usage sample"),
        StreamEvent::UsageDelta(
            Usage {
                reported: true,
                input_tokens: 7,
                output_tokens: 2,
                ..Usage::default()
            }
            .with_field_coverage(
                /*input_tokens*/ Some(0),
                /*output_tokens*/ Some(0)
            )
        )
    );
    assert!(stream.next().await.expect("terminal error").is_err());
    assert!(stream.next().await.is_none());
    server.await.expect("server task");
}

fn request() -> ModelRequest {
    ModelRequest {
        model_slug: ModelProfileKey::Generic,
        model: "test-model".to_string(),
        system: None,
        messages: Vec::new(),
        max_tokens: 16,
        tools: None,
        hosted_tools: Vec::new(),
        sampling: Default::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    }
}

async fn serve(body: String, content_type: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut request = Vec::new();
        loop {
            let mut buffer = [0_u8; 4096];
            let read = socket.read(&mut buffer).await.expect("read request");
            assert!(read > 0, "request ended before the body");
            request.extend_from_slice(&buffer[..read]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let length = body.len();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("response");
        socket.shutdown().await.expect("shutdown");
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn partial_payloads_preserve_unknown_primary_fields_independently_from_total() {
    use devo_protocol::native::usage::UsageField;
    for family in [
        Family::Chat,
        Family::Responses,
        Family::Anthropic,
        Family::Google,
    ] {
        let (input, output, total) = match family {
            Family::Chat => ("prompt_tokens", "completion_tokens", "total_tokens"),
            Family::Responses | Family::Anthropic => {
                ("input_tokens", "output_tokens", "total_tokens")
            }
            Family::Google => (
                "promptTokenCount",
                "candidatesTokenCount",
                "totalTokenCount",
            ),
        };
        for mode in [Mode::Completion, Mode::Stream] {
            for field in [input, output, total] {
                if field == total && matches!(family, Family::Anthropic) {
                    continue;
                }
                let (body, content_type) = family.body(mode, Some(json!({field: 0})));
                let (url, server) = serve(body, content_type).await;
                let provider = family.provider(url);
                let usage = match mode {
                    Mode::Completion => provider.completion(request()).await.unwrap().usage,
                    Mode::Stream => {
                        let mut stream = provider.completion_stream(request()).await.unwrap();
                        let mut done = None;
                        while let Some(event) = stream.next().await {
                            if let StreamEvent::MessageDone { response } = event.unwrap() {
                                done = Some(response.usage);
                            }
                        }
                        done.unwrap()
                    }
                };
                assert!(usage.is_reported());
                let expected = UsageField::ALL
                    .into_iter()
                    .filter(|counter| {
                        !((*counter == UsageField::InputTokens && field == input)
                            || (*counter == UsageField::OutputTokens && field == output)
                            || (*counter == UsageField::TotalTokens && field == total))
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    usage.unreported_fields,
                    Some(expected),
                    "{family:?} {mode:?} {field}"
                );
                server.await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn anthropic_stream_fills_primary_coverage_without_erasing_start_fields() {
    use devo_protocol::native::usage::UsageField;
    let body = concat!(
        "event: message_start\ndata: {\"message\":{\"id\":\"x\",\"usage\":{\"input_tokens\":5,\"cache_read_input_tokens\":0}}}\n\n",
        "event: message_delta\ndata: {\"usage\":{\"output_tokens\":0}}\n\n",
        "event: message_stop\ndata: {}\n\n"
    );
    let (url, server) = serve(body.to_string(), "text/event-stream").await;
    let mut events = Family::Anthropic
        .provider(url)
        .completion_stream(request())
        .await
        .unwrap();
    let mut samples = Vec::new();
    while let Some(event) = events.next().await {
        match event.unwrap() {
            StreamEvent::UsageDelta(usage) => samples.push(usage),
            StreamEvent::MessageDone { response } => samples.push(response.usage),
            _ => {}
        }
    }
    assert_eq!(
        samples[0].unreported_fields,
        Some(vec![
            UsageField::OutputTokens,
            UsageField::ReasoningTokens,
            UsageField::CacheCreationInputTokens,
            UsageField::TotalTokens
        ])
    );
    assert_eq!(
        samples[1].unreported_fields,
        Some(vec![
            UsageField::ReasoningTokens,
            UsageField::CacheCreationInputTokens
        ])
    );
    assert_eq!(samples[1], samples[2]);
    server.await.unwrap();
}

#[tokio::test]
async fn completion_observer_preserves_usage_when_responses_fails() {
    let failure = json!({"response": {
        "status": "failed", "usage": {"input_tokens": 7, "output_tokens": 2},
        "error": {"code": "server_error", "message": "failed after generation"}
    }});
    let (url, server) = serve(
        format!("event: response.failed\ndata: {failure}\n\n"),
        "text/event-stream",
    )
    .await;
    let samples = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&samples);
    let result = Family::Responses
        .provider(url)
        .completion_with_usage_observer(
            request(),
            Arc::new(move |usage| observed.lock().unwrap().push(usage.clone())),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(
        *samples.lock().unwrap(),
        vec![
            Usage {
                reported: true,
                input_tokens: 7,
                output_tokens: 2,
                ..Usage::default()
            }
            .with_field_coverage(
                /*input_tokens*/ Some(7),
                /*output_tokens*/ Some(2)
            )
        ]
    );
    server.await.unwrap();
}

#[tokio::test]
async fn completion_observer_reports_each_responses_sample_and_final_snapshot() {
    let body = concat!(
        "data: {\"usage\":{\"input_tokens\":7,\"output_tokens\":1}}\n\n",
        "event: response.completed\ndata: {\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":7,\"output_tokens\":2}}}\n\n"
    );
    let (url, server) = serve(body.to_string(), "text/event-stream").await;
    let samples = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&samples);
    let response = Family::Responses
        .provider(url)
        .completion_with_usage_observer(
            request(),
            Arc::new(move |usage| observed.lock().unwrap().push(usage.clone())),
        )
        .await
        .unwrap();
    let expected = [1, 2]
        .into_iter()
        .map(|output_tokens| {
            Usage {
                reported: true,
                input_tokens: 7,
                output_tokens,
                ..Usage::default()
            }
            .with_field_coverage(/*input_tokens*/ Some(7), Some(output_tokens))
        })
        .collect::<Vec<_>>();
    assert_eq!(*samples.lock().unwrap(), expected);
    assert_eq!(response.usage, expected[1]);
    server.await.unwrap();
}

#[tokio::test]
async fn completion_observer_keeps_default_adapters_and_unknown_usage_compatible() {
    for family in [
        Family::Chat,
        Family::Responses,
        Family::Anthropic,
        Family::Google,
    ] {
        for usage in [None, Some(family.zero())] {
            let (body, content_type) = family.body(Mode::Completion, usage.clone());
            let (url, server) = serve(body, content_type).await;
            let samples = Arc::new(Mutex::new(Vec::new()));
            let observed = Arc::clone(&samples);
            let response = family
                .provider(url)
                .completion_with_usage_observer(
                    request(),
                    Arc::new(move |usage| observed.lock().unwrap().push(usage.clone())),
                )
                .await
                .unwrap();
            let expected = if usage.is_some() {
                vec![response.usage]
            } else {
                Vec::new()
            };
            assert_eq!(*samples.lock().unwrap(), expected, "{family:?}");
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn google_marks_total_unknown_when_thoughts_have_no_provider_total() {
    use devo_protocol::native::usage::UsageField;
    for thoughts in [0, 5] {
        let payload = json!({
            "promptTokenCount": 10, "candidatesTokenCount": 3,
            "thoughtsTokenCount": thoughts, "cachedContentTokenCount": 2
        });
        let (body, content_type) = Family::Google.body(Mode::Completion, Some(payload));
        let (url, server) = serve(body, content_type).await;
        let response = Family::Google
            .provider(url)
            .completion(request())
            .await
            .unwrap();
        assert_eq!(
            response.usage,
            Usage {
                reported: true,
                input_tokens: 10,
                output_tokens: 3,
                reasoning_output_tokens: Some(thoughts),
                cache_read_input_tokens: Some(2),
                unreported_fields: Some(vec![
                    UsageField::CacheCreationInputTokens,
                    UsageField::TotalTokens
                ]),
                ..Usage::default()
            }
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn routed_completion_observer_preserves_failed_responses_usage() {
    use devo_protocol::ProviderWireApi;
    use devo_provider::{MultiProviderRouter, ProviderRoute, ProviderRouter, SingleProviderRouter};

    #[derive(Debug, Clone, Copy)]
    enum Routing {
        Single,
        MultiDefault,
        MultiConnection,
    }

    for routing in [
        Routing::Single,
        Routing::MultiDefault,
        Routing::MultiConnection,
    ] {
        let failure = json!({"response": {
            "status": "failed", "usage": {"input_tokens": 7, "output_tokens": 2},
            "error": {"code": "server_error", "message": "failed after generation"}
        }});
        let (url, server) = serve(
            format!("event: response.failed\ndata: {failure}\n\n"),
            "text/event-stream",
        )
        .await;
        let provider = Arc::new(OpenAIResponsesProvider::new(url));
        let connection = ProviderRoute::connection_model(
            "selected",
            ProviderWireApi::OpenAIResponses,
            "test-model",
        );
        let (router, route): (Box<dyn ProviderRouter>, _) = match routing {
            // Single-provider dispatch deliberately ignores the selected route.
            Routing::Single => (Box::new(SingleProviderRouter::new(provider)), connection),
            Routing::MultiDefault => (
                Box::new(MultiProviderRouter::new(provider)),
                ProviderRoute::Default,
            ),
            Routing::MultiConnection => {
                // A fallback to the default adapter cannot supply the expected sample.
                let mut router = MultiProviderRouter::new(Arc::new(OpenAIResponsesProvider::new(
                    "http://127.0.0.1:1",
                )));
                router.insert_route(connection.clone(), provider);
                (Box::new(router), connection)
            }
        };
        let samples = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&samples);
        let result = router
            .complete_with_usage_observer(
                route,
                request(),
                Arc::new(move |usage| observed.lock().unwrap().push(usage.clone())),
            )
            .await;
        assert!(result.is_err(), "{routing:?}");
        assert_eq!(
            *samples.lock().unwrap(),
            vec![
                Usage {
                    reported: true,
                    input_tokens: 7,
                    output_tokens: 2,
                    ..Usage::default()
                }
                .with_field_coverage(
                    /*input_tokens*/ Some(7),
                    /*output_tokens*/ Some(2)
                )
            ],
            "{routing:?}"
        );
        server.await.unwrap();
    }
}
