//! Deterministic deadline tests at the real query retry boundary.
use super::*;
use crate::SessionConfig;
use async_trait::async_trait;
use devo_protocol::ModelProfileKey;
use futures::stream;
use pretty_assertions::assert_eq;
use std::time::Duration;

fn session() -> SessionState {
    SessionState::new(SessionConfig::default(), std::env::temp_dir())
}

fn context() -> ProviderStreamContext<'static> {
    ProviderStreamContext {
        provider_name: "test",
        model_slug: "test",
        deadline: tokio::time::Instant::now() + response_idle_timeout(),
    }
}

fn request() -> ModelRequest {
    ModelRequest {
        model_slug: ModelProfileKey::Generic,
        model: "test-model".into(),
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

fn assert_timeout(error: &anyhow::Error) {
    assert!(matches!(
        error.downcast_ref::<ProviderError>(),
        Some(ProviderError::ProviderTimeoutError { .. })
    ));
    assert!(matches!(
        super::super::provider_retry_decision(error, &mut 0, &mut false),
        super::super::provider_retry::ProviderRetryDecision::RetryAfter(_)
    ));
}

struct NeverCreates;
#[async_trait]
impl ModelProviderSDK for NeverCreates {
    async fn completion(&self, _: ModelRequest) -> anyhow::Result<ModelResponse> {
        unreachable!()
    }
    async fn completion_stream(&self, _: ModelRequest) -> anyhow::Result<ProviderEventStream> {
        std::future::pending().await
    }
    fn name(&self) -> &str {
        "never-creates"
    }
}

#[tokio::test(start_paused = true)]
async fn stream_creation_times_out_with_retryable_typed_error() {
    let result = tokio::time::timeout(
        Duration::from_secs(301),
        run_provider_attempt(
            &NeverCreates,
            request(),
            &mut session(),
            &None,
            None,
            "test",
        ),
    )
    .await
    .expect("creation must terminate by 300 seconds");
    let Err(ProviderAttemptError::Create(error)) = result else {
        panic!("expected create timeout")
    };
    assert_timeout(&error);
}

#[tokio::test(start_paused = true)]
async fn idle_before_content_times_out_and_is_retryable() {
    let result = tokio::time::timeout(
        Duration::from_secs(301),
        consume_provider_stream(
            Box::pin(stream::pending()),
            &mut session(),
            &None,
            None,
            context(),
        ),
    )
    .await
    .expect("idle stream must terminate by 300 seconds");
    let Err(ProviderAttemptError::Retryable(error)) = result else {
        panic!("expected retryable timeout")
    };
    assert_timeout(&error);
}

#[tokio::test(start_paused = true)]
async fn partial_content_or_tool_start_timeout_is_fatal_not_replayed() {
    for event in [
        StreamEvent::TextDelta {
            index: 0,
            text: "partial".into(),
        },
        StreamEvent::ReasoningDelta {
            index: 0,
            text: "thinking".into(),
        },
        StreamEvent::ToolCallStart {
            index: 0,
            id: "call".into(),
            name: "ipython".into(),
            input: serde_json::Value::Null,
        },
        StreamEvent::HostedToolCallStart {
            index: 0,
            id: "hosted".into(),
            name: "search".into(),
            input: serde_json::json!({}),
        },
    ] {
        let events = stream::once(async { Ok(event) }).chain(stream::pending());
        let result = tokio::time::timeout(
            Duration::from_secs(301),
            consume_provider_stream(Box::pin(events), &mut session(), &None, None, context()),
        )
        .await
        .expect("partial stream must terminate");
        let Err(ProviderAttemptError::Fatal(error)) = result else {
            panic!("partial content must never retry")
        };
        assert_timeout(&error);
    }
}

#[tokio::test(start_paused = true)]
async fn ongoing_output_resets_idle_deadline_without_overall_limit() {
    let events = async_stream::stream! {
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_secs(250)).await;
            yield Ok(StreamEvent::TextDelta { index: 0, text: "x".into() });
        }
    };
    let start = tokio::time::Instant::now();
    let result =
        consume_provider_stream(Box::pin(events), &mut session(), &None, None, context()).await;
    assert!(result.is_ok());
    assert_eq!(start.elapsed(), Duration::from_secs(1000));
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_idle_does_not_wait_for_timeout() {
    let token = CancellationToken::new();
    token.cancel();
    let start = tokio::time::Instant::now();
    let result = consume_provider_stream(
        Box::pin(stream::pending()),
        &mut session(),
        &None,
        Some(&token),
        context(),
    )
    .await;
    assert!(result.is_ok());
    assert_eq!(start.elapsed(), Duration::ZERO);
}

/// Exercises the actual HTTP adapters, including Responses' eager header wait.
#[tokio::test]
async fn real_http_stalled_headers_and_keepalive_only_streams_time_out() {
    use devo_provider::anthropic::AnthropicProvider;
    use devo_provider::openai::{OpenAIProvider, OpenAIResponsesProvider};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    for keepalive in [false, true] {
        for api in ["chat", "responses", "anthropic"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (received_tx, received_rx) = tokio::sync::oneshot::channel();
            let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0; 8192];
                assert!(socket.read(&mut buf).await.unwrap() > 0);
                if keepalive {
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n: heartbeat\n\n").await.unwrap();
                    socket.flush().await.unwrap();
                }
                received_tx.send(()).unwrap();
                let _ = stop_rx.await;
            });
            let provider: Box<dyn ModelProviderSDK> = match api {
                "chat" => Box::new(OpenAIProvider::new(base)),
                "responses" => Box::new(OpenAIResponsesProvider::new(base)),
                "anthropic" => Box::new(AnthropicProvider::new(base)),
                _ => unreachable!(),
            };
            let attempt = tokio::spawn(async move {
                run_provider_attempt(
                    provider.as_ref(),
                    request(),
                    &mut session(),
                    &None,
                    None,
                    "test",
                )
                .await
            });
            tokio::time::timeout(Duration::from_secs(10), received_rx)
                .await
                .expect("HTTP request reached server")
                .unwrap();
            // Let the real socket/read machinery process the response before
            // advancing the query deadline; no shortened production timeout.
            tokio::time::sleep(Duration::from_millis(50)).await;
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(301)).await;
            let result = tokio::time::timeout(Duration::from_secs(1), attempt)
                .await
                .expect("stalled HTTP attempt must terminate")
                .unwrap();
            tokio::time::resume();
            let error = match result {
                Err(
                    ProviderAttemptError::Create(error) | ProviderAttemptError::Retryable(error),
                ) => error,
                _ => panic!("expected no-content HTTP timeout for {api}, keepalive={keepalive}"),
            };
            assert_timeout(&error);
            let _ = stop_tx.send(());
            server.await.unwrap();
        }
    }
}

struct LateHeaders;
#[async_trait]
impl ModelProviderSDK for LateHeaders {
    async fn completion(&self, _: ModelRequest) -> anyhow::Result<ModelResponse> {
        unreachable!()
    }
    async fn completion_stream(&self, _: ModelRequest) -> anyhow::Result<ProviderEventStream> {
        tokio::time::sleep(Duration::from_secs(299)).await;
        Ok(Box::pin(stream::pending()))
    }
    fn name(&self) -> &str {
        "late-headers"
    }
}

#[tokio::test(start_paused = true)]
async fn stream_creation_and_first_event_share_one_deadline() {
    let start = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(301),
        run_provider_attempt(&LateHeaders, request(), &mut session(), &None, None, "test"),
    )
    .await
    .expect("headers must not reset first-event deadline");
    let Err(ProviderAttemptError::Retryable(error)) = result else {
        panic!("expected idle timeout after headers")
    };
    assert_timeout(&error);
    assert_eq!(start.elapsed(), Duration::from_secs(300));
}

#[tokio::test(start_paused = true)]
async fn slow_local_callbacks_do_not_consume_the_next_idle_budget() {
    let events = async_stream::stream! {
        for _ in 0..2 {
            tokio::time::sleep(Duration::from_secs(250)).await;
            yield Ok(StreamEvent::TextDelta { index: 0, text: "x".into() });
        }
    };
    let callback: EventCallback = std::sync::Arc::new(|event| {
        Box::pin(async move {
            if matches!(event, QueryEvent::TextDelta(_)) {
                tokio::time::sleep(Duration::from_secs(350)).await;
            }
        })
    });
    let start = tokio::time::Instant::now();
    let result = consume_provider_stream(
        Box::pin(events),
        &mut session(),
        &Some(callback),
        None,
        context(),
    )
    .await;
    assert!(result.is_ok());
    assert_eq!(start.elapsed(), Duration::from_secs(1200));
}
