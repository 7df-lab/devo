//! Routing must not erase structured failures before retry classification.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use devo_protocol::{ModelProfileKey, ModelRequest, ModelResponse, ProviderWireApi, StreamEvent};
use devo_provider::ModelProviderSDK;
use devo_provider::error::ProviderError;
use devo_provider::router::{
    MultiProviderRouter, ProviderRoute, ProviderRouter, SingleProviderRouter,
};
use futures::Stream;
use pretty_assertions::assert_eq;

struct FailingProvider(ProviderError);

#[async_trait]
impl ModelProviderSDK for FailingProvider {
    async fn completion(&self, _request: ModelRequest) -> anyhow::Result<ModelResponse> {
        Err(anyhow::Error::new(self.0.clone()))
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        Err(anyhow::Error::new(self.0.clone()))
    }

    fn name(&self) -> &str {
        "failing-provider"
    }
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

#[tokio::test]
async fn both_routers_preserve_errors_across_all_invocation_surfaces() {
    let errors = [
        ProviderError::QuotaExceededError {
            message: "HTTP 429: insufficient_quota".to_string(),
            provider_name: Some("provider".to_string()),
        },
        ProviderError::RateLimitError {
            message: "slow down".to_string(),
            retry_after_seconds: Some(7),
            provider_name: Some("provider".to_string()),
        },
        ProviderError::AuthenticationError {
            message: "denied".to_string(),
            provider_name: Some("provider".to_string()),
            status_code: Some(403),
        },
        ProviderError::ContextLimitError {
            message: "too large".to_string(),
            current_tokens: Some(100),
            limit: Some(80),
        },
        ProviderError::ProviderServerError {
            message: "busy".to_string(),
            status_code: Some(503),
            provider_name: Some("provider".to_string()),
        },
    ];
    for expected in errors {
        let provider = Arc::new(FailingProvider(expected.clone()));
        let single = SingleProviderRouter::new(provider.clone());
        let route = ProviderRoute::connection("test", ProviderWireApi::OpenAIResponses);
        let mut multi = MultiProviderRouter::new(provider.clone());
        multi.insert_route(route.clone(), provider);
        let routers: [(&dyn ProviderRouter, ProviderRoute); 3] = [
            (&single, ProviderRoute::Default),
            (&multi, ProviderRoute::Default),
            (&multi, route),
        ];
        for (router, route) in routers {
            let complete = router.complete(route.clone(), request()).await.unwrap_err();
            let observed = router
                .complete_with_usage_observer(
                    route.clone(),
                    request(),
                    Arc::new(|_| panic!("failed completion must not invent usage")),
                )
                .await
                .unwrap_err();
            let stream = match router.stream(route, request()).await {
                Ok(_) => panic!("stream setup must fail"),
                Err(error) => error,
            };
            for actual in [complete, observed, stream] {
                assert_eq!(
                    serde_json::to_value(&actual).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
                assert_eq!(actual.is_recoverable(), expected.is_recoverable());
                assert_eq!(actual.retry_after_seconds(), expected.retry_after_seconds());
            }
        }
    }
}

#[tokio::test]
async fn responses_http_quota_failure_survives_routing() {
    use devo_provider::openai::OpenAIResponsesProvider;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0, "request ended before its body");
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
        let body = r#"{"error":{"type":"insufficient_quota","message":"quota exhausted"}}"#;
        let length = body.len();
        let response = format!(
            "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    let provider = Arc::new(OpenAIResponsesProvider::new(format!("http://{address}")));
    let router = MultiProviderRouter::new(provider);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        router.stream(ProviderRoute::Default, request()),
    )
    .await
    .expect("local HTTP response");
    let error = match result {
        Ok(_) => panic!("HTTP failure must fail stream setup"),
        Err(error) => error,
    };
    assert!(matches!(error, ProviderError::QuotaExceededError { .. }));
    assert!(!error.is_recoverable());
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("429"), "{diagnostic}");
    assert!(diagnostic.contains("insufficient_quota"), "{diagnostic}");
    assert!(diagnostic.contains("test-model"), "{diagnostic}");
    server.await.unwrap();
}
