use std::pin::Pin;

use devo_core::{Model, ProviderRequestModelMap};
use devo_protocol::{ModelResponse, StreamEvent};
use futures::Stream;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::*;

fn hosted(capability: Capability, input: Value, output: Option<Value>) -> ResponseContent {
    ResponseContent::HostedToolUse {
        id: "call".into(),
        name: capability.name().into(),
        input,
        output,
        status: Some("completed".into()),
    }
}

#[test]
fn capability_evidence_is_specific_to_each_tool() {
    let search = hosted(
        Capability::Search,
        json!({"query":"IANA example domains"}),
        None,
    );
    let open_page = hosted(
        Capability::Search,
        json!({"type":"open_page","url":"https://example.com/"}),
        None,
    );
    let fetch = hosted(
        Capability::Fetch,
        json!({}),
        Some(json!({"type":"web_fetch_result","url":"https://example.com/"})),
    );
    let rejected = hosted(
        Capability::Fetch,
        json!({}),
        Some(json!({"type":"web_fetch_tool_result_error","error_code":"tool_not_enabled"})),
    );
    let limited = hosted(
        Capability::Fetch,
        json!({}),
        Some(json!({"type":"web_fetch_tool_result_error","error_code":"too_many_requests"})),
    );
    let plain = ResponseContent::Text("I searched the web.".into());
    for (capability, blocks, expected) in [
        (Capability::Search, vec![search], ProbeResult::Supported),
        (
            Capability::Fetch,
            vec![open_page],
            ProbeResult::Inconclusive,
        ),
        (Capability::Fetch, vec![fetch], ProbeResult::Supported),
        (Capability::Fetch, vec![rejected], ProbeResult::Rejected),
        (Capability::Fetch, vec![limited], ProbeResult::Inconclusive),
        (Capability::Search, vec![plain], ProbeResult::Inconclusive),
        (
            Capability::Search,
            vec![hosted(Capability::Search, json!({}), None)],
            ProbeResult::Inconclusive,
        ),
    ] {
        assert_eq!(
            classify_response(ProviderWireApi::OpenAIResponses, capability, &blocks),
            expected
        );
    }
    assert_eq!(
        classify_response(
            ProviderWireApi::AnthropicMessages,
            Capability::Search,
            &[hosted(Capability::Search, json!({}), Some(json!([]))),]
        ),
        ProbeResult::Supported,
        "an empty but valid search result still verifies the tool"
    );
}

#[test]
fn explicit_rejections_are_not_confused_with_connection_or_quota_errors() {
    let explicit = ProviderError::InvalidRequestError {
        message: "web_fetch_20260209 is not supported on this endpoint".into(),
        details: None,
    };
    let rate_limited = ProviderError::RateLimitError {
        message: "web_fetch is not allowed until the rate limit resets".into(),
        retry_after_seconds: None,
        provider_name: None,
    };
    let timeout = ProviderError::ProviderTimeoutError {
        message: "web_fetch timed out".into(),
        provider_name: None,
    };
    assert_eq!(
        [
            explicitly_rejected(Capability::Fetch, &explicit),
            explicitly_rejected(Capability::Search, &explicit),
            explicitly_rejected(Capability::Fetch, &rate_limited),
            explicitly_rejected(Capability::Fetch, &timeout),
        ],
        [true, false, false, false]
    );
}

struct TestRouter(mpsc::UnboundedSender<(ProviderRoute, ModelRequest)>);

#[async_trait::async_trait]
impl ProviderRouter for TestRouter {
    fn has_available_route(&self, _route: &ProviderRoute) -> bool {
        true
    }

    async fn stream(
        &self,
        _: ProviderRoute,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>, ProviderError>
    {
        panic!("capability probes should use complete")
    }

    async fn complete(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        let _ = self.0.send((route, request));
        Ok(ModelResponse {
            id: "probe".into(),
            content: vec![hosted(
                Capability::Search,
                json!({"query":"IANA example domains"}),
                Some(json!([])),
            )],
            stop_reason: None,
            usage: Default::default(),
            metadata: Default::default(),
        })
    }

    fn name(&self) -> &str {
        "probe-test"
    }
}

#[tokio::test]
async fn probe_uses_the_selected_connection_and_only_public_inputs_then_caches_per_router() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let router: Arc<dyn ProviderRouter> = Arc::new(TestRouter(tx.clone()));
    let route = ProviderRoute::connection("example", ProviderWireApi::AnthropicMessages);
    let config = TurnConfig::with_provider_route(
        Model {
            slug: "example/model".into(),
            provider: ProviderWireApi::AnthropicMessages,
            ..Model::default()
        },
        "real-wire-model".into(),
        ProviderRequestModelMap::default()
            .with_request_config(Some(json!({"messages":["private"]})), Default::default()),
        route.clone(),
        None,
    );
    let checks = HostedWebChecks::default();
    assert_eq!(
        checks.inspect(
            Arc::clone(&router),
            &config,
            Capability::Search,
            ProbePolicy::CacheOnly
        ),
        None
    );
    assert!(
        checks.entries.lock().unwrap().is_empty(),
        "status reads must not schedule probes"
    );
    for _ in 0..2 {
        assert_eq!(
            checks.inspect(
                Arc::clone(&router),
                &config,
                Capability::Search,
                ProbePolicy::ScheduleIfNeeded
            ),
            None
        );
    }
    let (actual_route, request) = rx.recv().await.expect("probe request");
    assert_eq!(actual_route, route);
    assert_eq!(request.model, "real-wire-model");
    assert_eq!(
        serde_json::to_value(&request.messages).unwrap(),
        json!([
            {"role":"user","content":[{"type":"text","text":Capability::Search.prompt()}]},
        ])
    );
    assert_eq!(request.hosted_tools, vec![Capability::Search.definition()]);
    assert!(request.tools.is_none() && request.extra_body.is_none());

    // Reacquiring every permit waits for the worker to publish its result.
    drop(checks.slots.acquire_many(4).await.unwrap());
    assert_eq!(
        checks.inspect(
            Arc::clone(&router),
            &config,
            Capability::Search,
            ProbePolicy::CacheOnly
        ),
        Some(ProbeResult::Supported)
    );
    assert!(rx.try_recv().is_err());

    let reloaded_router: Arc<dyn ProviderRouter> = Arc::new(TestRouter(tx));
    assert_eq!(
        checks.inspect(
            Arc::clone(&reloaded_router),
            &config,
            Capability::Search,
            ProbePolicy::ScheduleIfNeeded
        ),
        None
    );
    assert_eq!(rx.recv().await.expect("probe after router reload").0, route);
    assert_eq!(
        checks.inspect(
            reloaded_router,
            &config,
            Capability::Fetch,
            ProbePolicy::ScheduleIfNeeded
        ),
        None
    );
    let (_, fetch_request) = rx.recv().await.expect("independent fetch probe");
    assert_eq!(
        fetch_request.hosted_tools,
        vec![Capability::Fetch.definition()]
    );
}
