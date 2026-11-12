use super::*;
use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn responses_failed_completion_persists_real_http_usage_before_returning_error() {
    let fixture = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut input = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            input.extend_from_slice(&buffer[..count]);
            if let Some(end) = input.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&input[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if input.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let body = "event: response.failed\ndata: {\"response\":{\"status\":\"failed\",\"usage\":{\"input_tokens\":100,\"output_tokens\":25,\"total_tokens\":125},\"error\":{\"code\":\"server_error\",\"message\":\"failed after work\"}}}\n\n";
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
    });
    let deps = crate::test_support::TestRuntime::new(Arc::new(
        devo_provider::openai::OpenAIResponsesProvider::new(format!("http://{address}")),
    ))
    .deps(fixture._temp.path());
    let provider = fixture.ledger.instrumented_provider(
        deps.process_context
            .provider_for_route(devo_provider::ProviderRoute::Default),
        fixture.session.id,
        Some(TurnId::new()),
        UsagePurpose::Compaction,
    );
    assert!(provider.completion(request()).await.is_err());
    server.await.unwrap();
    let live = fixture.ledger.session_usage(fixture.session.id).unwrap();
    assert_eq!(live.total.call_count, 1);
    assert_eq!(live.total.metered_call_count, 1);
    assert_eq!(live.total.failed_call_count, 1);
    assert_eq!(live.total.total_tokens, 125);
    assert_eq!(
        fixture.replay().session_usage(fixture.session.id).unwrap(),
        live
    );
}

struct ObservedPendingCompletion {
    observed: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl ModelProviderSDK for ObservedPendingCompletion {
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        unreachable!()
    }
    async fn completion_with_usage_observer(
        &self,
        _: ModelRequest,
        observer: devo_provider::UsageObserver,
    ) -> Result<ModelResponse> {
        observer(&reported(20, 3));
        self.observed.notify_one();
        std::future::pending().await
    }
    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        unreachable!()
    }
    fn name(&self) -> &str {
        "pending-completion"
    }
}

#[tokio::test]
async fn cancelled_completion_keeps_the_observed_sample_and_cancellation_outcome() {
    let fixture = Fixture::new();
    let observed = Arc::new(tokio::sync::Notify::new());
    let deps = crate::test_support::TestRuntime::new(Arc::new(ObservedPendingCompletion {
        observed: observed.clone(),
    }))
    .deps(fixture._temp.path());
    let provider = fixture.ledger.instrumented_provider(
        deps.process_context
            .provider_for_route(devo_provider::ProviderRoute::Default),
        fixture.session.id,
        None,
        UsagePurpose::TitleGeneration,
    );
    let mut completion = Box::pin(provider.completion(request()));
    tokio::select! {
        _ = observed.notified() => {},
        result = &mut completion => panic!("completion unexpectedly ended: {result:?}"),
    }
    drop(completion);
    let live = fixture.ledger.session_usage(fixture.session.id).unwrap();
    assert_eq!(live.total.call_count, 1);
    assert_eq!(live.total.cancelled_call_count, 1);
    assert_eq!(live.total.metered_call_count, 1);
    assert_eq!(live.total.total_tokens, 23);
    assert_eq!(
        fixture.replay().session_usage(fixture.session.id).unwrap(),
        live
    );
}

struct ProvisionalZeroCompletion;
#[async_trait]
impl ModelProviderSDK for ProvisionalZeroCompletion {
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        unreachable!()
    }
    async fn completion_with_usage_observer(
        &self,
        _: ModelRequest,
        observer: devo_provider::UsageObserver,
    ) -> Result<ModelResponse> {
        observer(&reported(0, 0));
        Ok(response(Usage::default()))
    }
    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        unreachable!()
    }
    fn name(&self) -> &str {
        "provisional-completion"
    }
}

#[tokio::test]
async fn completion_observer_does_not_promote_provisional_zero() {
    let f = Fixture::new();
    let provider = f.ledger.instrumented_provider(
        Arc::new(ProvisionalZeroCompletion),
        f.session.id,
        None,
        UsagePurpose::Compaction,
    );
    let result = provider.completion(request()).await.unwrap();
    assert!(!result.usage.is_reported());
    let total = f.ledger.session_usage(f.session.id).unwrap().total;
    assert_eq!(total.call_count, 1);
    assert_eq!(total.metered_call_count, 0);
}

async fn serve_terminal_usage(body: String) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut input = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            input.extend_from_slice(&buffer[..count]);
            if let Some(end) = input.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&input[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if input.len() >= end + 4 + length {
                    break;
                }
            }
        }
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
    });
    format!("http://{address}")
}

#[tokio::test]
async fn explicit_failed_terminal_zero_replaces_positive_progress_for_both_entrypoints() {
    for event in [
        "error",
        "response.failed",
        "response.completed",
        "response.done",
        "parse-error",
    ] {
        for completion in [false, true] {
            let f = Fixture::new();
            let zero = serde_json::json!({"input_tokens":0,"output_tokens":0});
            let failure = serde_json::json!({"code":"server_error","message":"failed"});
            let terminal = match event {
                "error" => serde_json::json!({"error":failure,"usage":zero}),
                "parse-error" => {
                    serde_json::json!({"response":{"id":"r","status":"completed","usage":zero,"output":[{"type":"function_call","id":"call","call_id":"call","name":"tool","arguments":"not json"}]}})
                }
                _ => {
                    serde_json::json!({"response":{"status":"failed","error":failure,"usage":zero}})
                }
            };
            let wire_event = if event == "parse-error" {
                "response.completed"
            } else {
                event
            };
            let body = format!(
                "event: response.in_progress\ndata: {{\"usage\":{{\"input_tokens\":7,\"output_tokens\":2}}}}\n\nevent: {wire_event}\ndata: {terminal}\n\n"
            );
            let url = serve_terminal_usage(body).await;
            let provider = f.ledger.instrumented_provider(
                Arc::new(devo_provider::openai::OpenAIResponsesProvider::new(url)),
                f.session.id,
                None,
                UsagePurpose::Compaction,
            );
            if completion {
                assert!(provider.completion(request()).await.is_err(), "{event}");
            } else {
                let events = provider
                    .completion_stream(request())
                    .await
                    .unwrap()
                    .collect::<Vec<_>>()
                    .await;
                assert!(events.iter().any(Result::is_err), "{event}");
            }
            let total = f.ledger.session_usage(f.session.id).unwrap().total;
            assert_eq!(total.call_count, 1, "{event}");
            assert_eq!(total.failed_call_count, 1, "{event}");
            assert_eq!(total.metered_call_count, 1, "{event}");
            assert_eq!(total.total_tokens, 0, "{event}");
            assert_eq!(total.input_tokens, 0, "{event}");
            assert_eq!(total.output_tokens, 0, "{event}");
        }
    }
}
