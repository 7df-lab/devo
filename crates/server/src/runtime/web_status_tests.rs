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
fn hosted_web_evidence_requires_the_actual_tool_and_action() {
    use Capability::*;
    use ProbeResult::*;
    let search = hosted(Search, json!({"query":"IANA example domains"}), None);
    let open = hosted(Search, json!({"type":"open_page","url":PROBE_URL}), None);
    let fetch = hosted(
        Fetch,
        json!({"url":PROBE_URL}),
        Some(json!({"type":"web_fetch_result","url":PROBE_URL})),
    );
    for (capability, block, expected) in [
        (Search, search.clone(), Supported),
        (OpenPage, search, Inconclusive),
        (OpenPage, open.clone(), Supported),
        (Search, open.clone(), Inconclusive),
        (Fetch, open, Inconclusive),
        (Fetch, fetch, Supported),
        (
            OpenPage,
            hosted(
                Search,
                json!({"type":"open_page","url":"https://example.org/"}),
                None,
            ),
            Inconclusive,
        ),
        (
            OpenPage,
            hosted(
                Search,
                json!({"type":"open_page","url":PROBE_URL}),
                Some(json!({"error":"fetch failed"})),
            ),
            Inconclusive,
        ),
        (
            Fetch,
            hosted(
                Fetch,
                json!({}),
                Some(json!({"error_code":"tool_not_enabled"})),
            ),
            Rejected,
        ),
        (
            Fetch,
            hosted(
                Fetch,
                json!({}),
                Some(json!({"error_code":"too_many_requests"})),
            ),
            Inconclusive,
        ),
        (
            Fetch,
            ResponseContent::Text("I fetched the page.".into()),
            Inconclusive,
        ),
        (
            Fetch,
            hosted(Fetch, json!({"url":PROBE_URL}), None),
            Inconclusive,
        ),
    ] {
        assert_eq!(
            classify_response(ProviderWireApi::OpenAIResponses, capability, &[block]),
            expected
        );
    }
    assert_eq!(
        classify_response(
            ProviderWireApi::AnthropicMessages,
            Search,
            &[hosted(Search, json!({}), Some(json!([])))]
        ),
        Supported,
        "an empty but valid search result still verifies the tool"
    );
}

#[test]
fn hosted_web_rejections_are_not_confused_with_quota_errors() {
    let explicit = ProviderError::InvalidRequestError {
        message: "web_fetch_20260209 is not supported on this endpoint".into(),
        details: None,
    };
    let rate_limited = ProviderError::RateLimitError {
        message: "web_fetch is not allowed until the rate limit resets".into(),
        retry_after_seconds: None,
        provider_name: None,
    };
    assert_eq!(
        [
            explicitly_rejected(Capability::Fetch, &explicit),
            explicitly_rejected(Capability::Search, &explicit),
            explicitly_rejected(Capability::Fetch, &rate_limited),
        ],
        [true, false, false]
    );
}

#[test]
fn hosted_web_picker_distinguishes_partial_and_unknown_support() {
    use HostedSupport::*;
    assert_eq!(
        [
            hosted_picker_note(Supported, Supported),
            hosted_picker_note(Supported, Unavailable),
            hosted_picker_note(Unavailable, Supported),
            hosted_picker_note(Supported, Unverified),
            hosted_picker_note(Checking, Checking),
            hosted_picker_note(Unavailable, Unavailable),
        ],
        [
            "Search + fetch",
            "Search only",
            "Fetch only",
            "Search supported · Fetch unverified",
            "Checking…",
            "Not supported"
        ]
    );
}

struct TestRouter {
    requests: mpsc::UnboundedSender<(ProviderRoute, ModelRequest)>,
    replies: Mutex<Vec<std::vec::IntoIter<ResponseContent>>>,
}

impl TestRouter {
    fn new(
        requests: mpsc::UnboundedSender<(ProviderRoute, ModelRequest)>,
        replies: Vec<Vec<ResponseContent>>,
    ) -> Arc<dyn ProviderRouter> {
        Arc::new(Self {
            requests,
            replies: Mutex::new(replies.into_iter().rev().map(Vec::into_iter).collect()),
        })
    }
}

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
        let _ = self.requests.send((route, request));
        let content = self
            .replies
            .lock()
            .unwrap()
            .pop()
            .expect("unexpected probe")
            .collect();
        Ok(ModelResponse {
            id: "test-response".into(),
            content,
            stop_reason: None,
            usage: Default::default(),
            metadata: Default::default(),
        })
    }

    fn name(&self) -> &str {
        "probe-test"
    }
}

fn turn_config(wire: ProviderWireApi) -> TurnConfig {
    TurnConfig::with_provider_route(
        Model {
            slug: "example/model".into(),
            provider: wire,
            ..Model::default()
        },
        "real-wire-model".into(),
        ProviderRequestModelMap::default()
            .with_request_config(Some(json!({"messages":["private"]})), Default::default()),
        ProviderRoute::connection("example", wire),
        None,
    )
}

#[tokio::test]
async fn hosted_web_combines_actions_and_reuses_persisted_evidence_after_restart() {
    let home = tempfile::tempdir().unwrap();
    let checks = HostedWebChecks::open(home.path());
    let (requests, mut rx) = mpsc::unbounded_channel();
    let router = TestRouter::new(
        requests.clone(),
        vec![vec![
            hosted(
                Capability::Search,
                json!({"query":"IANA example domains"}),
                None,
            ),
            hosted(
                Capability::OpenPage,
                json!({"type":"open_page","url":PROBE_URL}),
                None,
            ),
        ]],
    );
    let config = turn_config(ProviderWireApi::OpenAIResponses);
    let identity = || Some("fixture-route-and-account".into());
    assert_eq!(
        checks.inspect(
            Arc::clone(&router),
            identity(),
            &config,
            ProbePolicy::CacheOnly
        ),
        ProbeReport::uniform(HostedSupport::Unverified)
    );
    for _ in 0..2 {
        checks.inspect(
            Arc::clone(&router),
            identity(),
            &config,
            ProbePolicy::ScheduleIfNeeded,
        );
    }
    let (route, request) = rx.recv().await.expect("model probe");
    assert_eq!(route, config.provider_route);
    assert_eq!(request.model, "real-wire-model");
    assert_eq!(request.hosted_tools, vec![Capability::Search.definition()]);
    assert_eq!(
        serde_json::to_value(request.messages).unwrap(),
        json!([
            {"role":"user","content":[{"type":"text","text":Capability::Search.prompt(/*also_open*/ true)}]}
        ])
    );
    assert!(request.tools.is_none() && request.extra_body.is_none());
    // Acquiring all permits waits for the worker, including its disk writes.
    drop(checks.slots.acquire_many(4).await.unwrap());
    assert!(
        rx.try_recv().is_err(),
        "the combined proof must not trigger a second request"
    );
    drop(checks);

    let restarted = HostedWebChecks::open(home.path());
    let other_router = TestRouter::new(requests, vec![]);
    assert_eq!(
        restarted.inspect(
            Arc::clone(&other_router),
            identity(),
            &config,
            ProbePolicy::ScheduleIfNeeded
        ),
        ProbeReport {
            search: HostedSupport::Supported,
            fetch: HostedSupport::Supported,
            standalone_fetch: HostedSupport::Unverified,
            fetch_via_search: true,
        }
    );
    assert!(
        rx.try_recv().is_err(),
        "restarting must not probe with fresh saved evidence"
    );
    assert_eq!(
        restarted.inspect(
            other_router,
            Some("different-credential".into()),
            &config,
            ProbePolicy::CacheOnly
        ),
        ProbeReport::uniform(HostedSupport::Unverified)
    );
}

#[tokio::test]
async fn hosted_web_tries_standalone_fetch_only_if_open_page_is_unproven() {
    let config = turn_config(ProviderWireApi::OpenAIResponses);
    let checks = HostedWebChecks::default();
    let (requests, mut rx) = mpsc::unbounded_channel();
    let router = TestRouter::new(
        requests,
        vec![
            vec![hosted(
                Capability::Search,
                json!({"query":"IANA example domains"}),
                None,
            )],
            vec![ResponseContent::Text("not a hosted action".into())],
            vec![hosted(
                Capability::Fetch,
                json!({"url":PROBE_URL}),
                Some(json!({"type":"web_fetch_result","url":PROBE_URL})),
            )],
        ],
    );
    checks.inspect(
        Arc::clone(&router),
        None,
        &config,
        ProbePolicy::ScheduleIfNeeded,
    );
    drop(checks.slots.acquire_many(4).await.unwrap());
    let requests: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .map(|(_, request)| request.hosted_tools)
        .collect();
    assert_eq!(
        requests,
        [
            vec![Capability::Search.definition()],
            vec![Capability::OpenPage.definition()],
            vec![Capability::Fetch.definition()]
        ]
    );
    assert_eq!(
        checks.inspect(router, None, &config, ProbePolicy::CacheOnly),
        ProbeReport {
            search: HostedSupport::Supported,
            fetch: HostedSupport::Supported,
            standalone_fetch: HostedSupport::Supported,
            fetch_via_search: false,
        }
    );
}
