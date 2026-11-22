//! Real, bounded hosted-tool probes. Evidence never switches web suppliers.

#[cfg(test)]
#[path = "web_status_tests.rs"]
mod tests;

#[path = "web_picker_status.rs"]
mod picker;

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use devo_core::{ResolvedWebFetchConfig, ResolvedWebSearchConfig, TurnConfig};
use devo_protocol::{
    HostedToolDefinition, HostedWebFetchTool, HostedWebSearchTool, ModelProfileKey, ModelRequest,
    ProviderWireApi, RequestContent, RequestMessage, ResponseContent,
};
use devo_provider::error::ProviderError;
use devo_provider::{ProviderRoute, ProviderRouter};
use lru::LruCache;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use super::web_probe_persistence::{WebProbeEvidence as ProbeResult, WebProbePersistence};
use crate::runtime::ServerRuntime;
use crate::session_context::SessionRuntimeContext;

const PROBE_TIMEOUT: Duration = Duration::from_secs(12);
const RESULT_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const INCONCLUSIVE_TTL: Duration = Duration::from_secs(3600);
// Bump when changing which wire requests or proofs a result represents.
const PROBE_REVISION: &str = "devo-hosted-web-v2";
const PROBE_URL: &str = "https://example.com/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Capability {
    Search,
    Fetch,
    OpenPage,
}

impl Capability {
    const ALL: [Self; 3] = [Self::Search, Self::Fetch, Self::OpenPage];

    fn name(self) -> &'static str {
        match self {
            Self::Search | Self::OpenPage => "web_search",
            Self::Fetch => "web_fetch",
        }
    }

    fn definition(self) -> HostedToolDefinition {
        match self {
            Self::Search | Self::OpenPage => HostedToolDefinition::WebSearch(HostedWebSearchTool {
                max_uses: Some(1),
                ..Default::default()
            }),
            Self::Fetch => HostedToolDefinition::WebFetch(HostedWebFetchTool {
                max_uses: Some(1),
                max_content_tokens: Some(256),
                ..Default::default()
            }),
        }
    }

    fn prompt(self, also_open: bool) -> String {
        match self {
            Self::Search if also_open => format!("Use hosted web search for the exact public query ‘IANA example domains’, then use its open_page action to open {PROBE_URL}. Do both tool actions before replying OK."),
            Self::Search => "Use hosted web search once for the exact public query ‘IANA example domains’. Then reply OK.".into(),
            Self::Fetch => format!("Use the standalone hosted web fetch tool once to fetch {PROBE_URL}. Then reply OK."),
            Self::OpenPage => format!("Use the hosted web search tool’s open_page action to open the exact URL {PROBE_URL}. Do not use a search query instead. Then reply OK."),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostedSupport {
    Supported,
    Unavailable,
    Checking,
    Unverified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProbeReport {
    search: HostedSupport,
    fetch: HostedSupport,
    standalone_fetch: HostedSupport,
    fetch_via_search: bool,
}

impl ProbeReport {
    fn uniform(support: HostedSupport) -> Self {
        Self {
            search: support,
            fetch: support,
            standalone_fetch: support,
            fetch_via_search: false,
        }
    }
}

#[derive(Clone, Copy)]
enum ProbePolicy {
    ScheduleIfNeeded,
    CacheOnly,
}

#[derive(Clone, Copy, Default)]
struct Evidence([Option<ProbeResult>; 3]);

impl Evidence {
    fn get(self, capability: Capability) -> Option<ProbeResult> {
        self.0[capability as usize]
    }

    fn set(&mut self, capability: Capability, result: ProbeResult) {
        self.0[capability as usize] = Some(result);
    }

    fn next(self, wire: ProviderWireApi, standalone_required: bool) -> Option<Capability> {
        use Capability::*;
        if self.get(Search).is_none() {
            return Some(Search);
        }
        if wire == ProviderWireApi::OpenAIResponses && !standalone_required {
            if self.get(Fetch) == Some(ProbeResult::Supported)
                || self.get(OpenPage) == Some(ProbeResult::Supported)
            {
                return None;
            }
            if self.get(Search) != Some(ProbeResult::Rejected) && self.get(OpenPage).is_none() {
                return Some(OpenPage);
            }
        }
        self.get(Fetch).is_none().then_some(Fetch)
    }

    fn report(self, wire: ProviderWireApi, pending: bool) -> ProbeReport {
        use Capability::*;
        use HostedSupport::*;
        let state = |value| match value {
            Some(ProbeResult::Supported) => Supported,
            Some(ProbeResult::Rejected) => Unavailable,
            Some(ProbeResult::Inconclusive) => Unverified,
            None if pending => Checking,
            None => Unverified,
        };
        let search = state(self.get(Search));
        let standalone_fetch = state(self.get(Fetch));
        let fetch_via_search = wire == ProviderWireApi::OpenAIResponses
            && standalone_fetch != Supported
            && self.get(OpenPage) == Some(ProbeResult::Supported);
        let fetch = if fetch_via_search || standalone_fetch == Supported {
            Supported
        } else if wire == ProviderWireApi::AnthropicMessages {
            standalone_fetch
        } else if pending && self.next(wire, /*standalone_required*/ false).is_some() {
            Checking
        } else if standalone_fetch == Unavailable
            && (search == Unavailable || self.get(OpenPage) == Some(ProbeResult::Rejected))
        {
            Unavailable
        } else {
            Unverified
        };
        ProbeReport {
            search,
            fetch,
            standalone_fetch,
            fetch_via_search,
        }
    }
}

struct Entry {
    // Unidentified routes can only reuse evidence inside the exact router snapshot.
    _router: Weak<dyn ProviderRouter>,
    results: [Option<(ProbeResult, Instant)>; 3],
    pending: bool,
}

#[derive(Clone)]
pub(super) struct HostedWebChecks {
    entries: Arc<Mutex<LruCache<String, Entry>>>,
    slots: Arc<Semaphore>,
    persisted: Option<Arc<WebProbePersistence>>,
}

impl Default for HostedWebChecks {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(128).unwrap()))),
            slots: Arc::new(Semaphore::new(4)),
            persisted: None,
        }
    }
}

struct ProbeRoute {
    key: String,
    durable: bool,
    route: ProviderRoute,
    wire: ProviderWireApi,
    request: ModelRequest,
    standalone_required: bool,
}

fn probe_record_key(scope: &str, capability: Capability) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{scope}:{}", capability as usize))
    )
}

impl HostedWebChecks {
    pub(super) fn open(home: &Path) -> Self {
        Self {
            persisted: Some(Arc::new(WebProbePersistence::open(home))),
            ..Self::default()
        }
    }

    fn inspect(
        &self,
        router: Arc<dyn ProviderRouter>,
        identity: Option<String>,
        config: &TurnConfig,
        policy: ProbePolicy,
    ) -> ProbeReport {
        let wire = match &config.provider_route {
            ProviderRoute::Connection { wire_api, .. } => *wire_api,
            ProviderRoute::Default => config.model.provider,
        };
        if !matches!(
            wire,
            ProviderWireApi::AnthropicMessages | ProviderWireApi::OpenAIResponses
        ) {
            return ProbeReport::uniform(HostedSupport::Unavailable);
        }
        if matches!(config.provider_route, ProviderRoute::Default)
            || !router.has_available_route(&config.provider_route)
        {
            return ProbeReport::uniform(HostedSupport::Unverified);
        }

        let reasoning = config
            .model
            .resolve_reasoning_effort_selection(config.reasoning_effort_selection.as_deref());
        // No history, user text, workspace context, or arbitrary body overrides.
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug(reasoning.request_model.clone()),
            model: config.provider_request_model(&reasoning.request_model),
            system: None,
            messages: Vec::new(),
            max_tokens: 256,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: Default::default(),
            request_thinking: reasoning.request_thinking,
            reasoning_effort: reasoning.request_reasoning_effort,
            extra_body: devo_core::add_model_request_headers(
                None,
                config.provider_request_models.request_headers(),
            ),
        };
        let durable = identity.is_some();
        let identity = identity.unwrap_or_else(|| format!("process:{:p}", Arc::as_ptr(&router)));
        let scope_route = format!("{:?}", config.provider_route);
        let candidate = serde_json::to_vec(&(
            PROBE_REVISION,
            identity,
            scope_route,
            &reasoning.request_model,
            &config.variant,
            &request,
        ));
        let Ok(scope) = candidate else {
            return ProbeReport::uniform(HostedSupport::Unverified);
        };
        let key = format!("{:x}", Sha256::digest(scope));
        let standalone_required =
            config.web_fetch.is_provider() && !config.web_search.is_provider();
        let mut evidence = Evidence::default();
        if durable && let Some(persisted) = &self.persisted {
            for capability in Capability::ALL {
                if let Some(result) = persisted.get(&probe_record_key(&key, capability)) {
                    evidence.set(capability, result);
                }
            }
        }
        let mut entries = self.entries.lock().expect("hosted web probe cache mutex");
        let now = Instant::now();
        let mut pending = false;
        if let Some(entry) = entries.get(&key) {
            for capability in Capability::ALL {
                if let Some((result, expires)) = entry.results[capability as usize]
                    && expires > now
                {
                    evidence.set(capability, result);
                }
            }
            pending = entry.pending;
        }
        if pending
            || evidence.next(wire, standalone_required).is_none()
            || matches!(policy, ProbePolicy::CacheOnly)
        {
            return evidence.report(wire, pending);
        }
        // Never queue unbounded work when users switch through many models.
        let Ok(slot) = Arc::clone(&self.slots).try_acquire_owned() else {
            return evidence.report(wire, /*pending*/ true);
        };
        if let Some(entry) = entries.get_mut(&key) {
            entry.pending = true;
        } else {
            entries.put(
                key.clone(),
                Entry {
                    _router: Arc::downgrade(&router),
                    results: [None; 3],
                    pending: true,
                },
            );
        }
        drop(entries);

        let checks = self.clone();
        let probe = ProbeRoute {
            key,
            durable,
            route: config.provider_route.clone(),
            wire,
            request,
            standalone_required,
        };
        tokio::spawn(async move {
            let _slot = slot;
            checks.run_strategy(router, probe, evidence).await;
        });
        evidence.report(wire, /*pending*/ true)
    }

    async fn run_strategy(
        &self,
        router: Arc<dyn ProviderRouter>,
        probe: ProbeRoute,
        mut evidence: Evidence,
    ) {
        while let Some(capability) = evidence.next(probe.wire, probe.standalone_required) {
            let mut request = probe.request.clone();
            let also_open = capability == Capability::Search
                && probe.wire == ProviderWireApi::OpenAIResponses
                && !probe.standalone_required
                && evidence.get(Capability::OpenPage).is_none()
                && evidence.get(Capability::Fetch) != Some(ProbeResult::Supported);
            request.messages = vec![RequestMessage {
                role: "user".into(),
                content: vec![RequestContent::Text {
                    text: capability.prompt(also_open),
                }],
            }];
            request.hosted_tools = vec![capability.definition()];
            let result = match tokio::time::timeout(
                PROBE_TIMEOUT,
                router.complete(probe.route.clone(), request),
            )
            .await
            {
                Ok(Ok(response)) => {
                    // One search request can confirm both actions. Never turn a
                    // missing second action into negative evidence; try it alone.
                    if also_open
                        && classify_response(probe.wire, Capability::OpenPage, &response.content)
                            == ProbeResult::Supported
                    {
                        evidence.set(Capability::OpenPage, ProbeResult::Supported);
                        self.record(&probe, Capability::OpenPage, ProbeResult::Supported)
                            .await;
                    }
                    classify_response(probe.wire, capability, &response.content)
                }
                Ok(Err(error)) if explicitly_rejected(capability, &error) => ProbeResult::Rejected,
                Ok(Err(_)) | Err(_) => ProbeResult::Inconclusive,
            };
            evidence.set(capability, result);
            self.record(&probe, capability, result).await;
        }
        if let Some(entry) = self
            .entries
            .lock()
            .expect("hosted web probe cache mutex")
            .get_mut(&probe.key)
        {
            entry.pending = false;
        }
    }

    async fn record(&self, probe: &ProbeRoute, capability: Capability, result: ProbeResult) {
        let ttl = match result {
            ProbeResult::Inconclusive => INCONCLUSIVE_TTL,
            ProbeResult::Supported | ProbeResult::Rejected => RESULT_TTL,
        };
        if let Some(entry) = self
            .entries
            .lock()
            .expect("hosted web probe cache mutex")
            .get_mut(&probe.key)
        {
            entry.results[capability as usize] = Some((result, Instant::now() + ttl));
        }
        if probe.durable
            && let Some(persisted) = &self.persisted
        {
            let persisted = Arc::clone(persisted);
            let key = probe_record_key(&probe.key, capability);
            let _ = tokio::task::spawn_blocking(move || persisted.put(key, result, ttl)).await;
        }
    }
}

fn classify_response(
    wire: ProviderWireApi,
    capability: Capability,
    content: &[ResponseContent],
) -> ProbeResult {
    let mut outcome = ProbeResult::Inconclusive;
    for block in content {
        let ResponseContent::HostedToolUse {
            name,
            input,
            output,
            status,
            ..
        } = block
        else {
            continue;
        };
        if name != capability.name() {
            continue;
        }
        let action = input.get("type").and_then(serde_json::Value::as_str);
        if capability == Capability::OpenPage && action != Some("open_page") {
            continue;
        }
        if let Some(error) = output
            .as_ref()
            .and_then(|value| value.get("error_code").and_then(serde_json::Value::as_str))
        {
            if matches!(
                error,
                "not_supported" | "tool_not_supported" | "tool_not_enabled" | "permission_denied"
            ) {
                outcome = ProbeResult::Rejected;
            }
            continue;
        }
        if status.as_deref() != Some("completed")
            || output
                .as_ref()
                .is_some_and(|value| value.get("error").is_some())
        {
            continue;
        }
        let supported = match capability {
            Capability::Search => {
                action != Some("open_page")
                    && (output.as_ref().is_some_and(serde_json::Value::is_array)
                        || (wire == ProviderWireApi::OpenAIResponses
                            && (action == Some("search")
                                || input
                                    .get("query")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some())))
            }
            Capability::Fetch => {
                output
                    .as_ref()
                    .and_then(|value| value.get("type").and_then(serde_json::Value::as_str))
                    == Some("web_fetch_result")
            }
            Capability::OpenPage => {
                wire == ProviderWireApi::OpenAIResponses
                    && input
                        .get("url")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|url| reqwest::Url::parse(url).ok())
                        .is_some_and(|url| url.as_str() == PROBE_URL)
            }
        };
        if supported {
            return ProbeResult::Supported;
        }
    }
    outcome
}

fn explicitly_rejected(capability: Capability, error: &ProviderError) -> bool {
    let message = match error {
        ProviderError::InvalidRequestError { message, .. }
        | ProviderError::UnknownError {
            message,
            status_code: Some(400 | 403 | 422),
        } => message.to_ascii_lowercase(),
        _ => return false,
    };
    (message.contains(capability.name())
        || (capability == Capability::OpenPage && message.contains("open_page")))
        && [
            "not supported",
            "unsupported",
            "not enabled",
            "not allowed",
            "unknown tool",
        ]
        .iter()
        .any(|reason| message.contains(reason))
}

fn hosted_status_note(state: HostedSupport) -> Option<String> {
    match state {
        HostedSupport::Supported => None,
        HostedSupport::Unavailable => Some("Not supported".into()),
        HostedSupport::Checking => Some("Checking…".into()),
        HostedSupport::Unverified => Some("Couldn’t verify".into()),
    }
}

fn hosted_picker_note(search: HostedSupport, fetch: HostedSupport) -> String {
    use HostedSupport::*;
    match (search, fetch) {
        (Supported, Supported) => "Search + fetch".into(),
        (Supported, Unavailable) => "Search only".into(),
        (Unavailable, Supported) => "Fetch only".into(),
        (Unavailable, Unavailable) => "Not supported".into(),
        (Checking, Checking) => "Checking…".into(),
        (Unverified, Unverified) => "Couldn’t verify".into(),
        _ => {
            let label = |capability, state| match state {
                Supported => format!("{capability} supported"),
                Unavailable => format!("{capability} unavailable"),
                Checking => format!("{capability} checking…"),
                Unverified => format!("{capability} unverified"),
            };
            format!("{} · {}", label("Search", search), label("Fetch", fetch))
        }
    }
}

fn web_probe_policy(config: &TurnConfig) -> ProbePolicy {
    if matches!(config.web_search, ResolvedWebSearchConfig::Disabled)
        && matches!(config.web_fetch, ResolvedWebFetchConfig::Disabled)
    {
        ProbePolicy::CacheOnly
    } else {
        ProbePolicy::ScheduleIfNeeded
    }
}

impl ServerRuntime {
    fn hosted_web_report(
        &self,
        context: &SessionRuntimeContext,
        config: &TurnConfig,
        policy: ProbePolicy,
    ) -> ProbeReport {
        let (router, identity) = context.hosted_web_probe_snapshot(&config.provider_route);
        self.hosted_web_checks
            .inspect(router, identity, config, policy)
    }

    pub(super) fn check_hosted_web(&self, context: &SessionRuntimeContext, config: &TurnConfig) {
        if matches!(web_probe_policy(config), ProbePolicy::ScheduleIfNeeded) {
            self.hosted_web_report(context, config, ProbePolicy::ScheduleIfNeeded);
        }
    }

    pub(in crate::runtime) fn suppress_rejected_hosted_tools(
        &self,
        context: &SessionRuntimeContext,
        config: &mut TurnConfig,
    ) {
        let report = self.hosted_web_report(context, config, ProbePolicy::CacheOnly);
        if config.web_fetch.is_provider() {
            if config.web_search.is_provider() && report.fetch_via_search {
                config.web_fetch = ResolvedWebFetchConfig::ProviderViaSearch;
            } else if report.standalone_fetch == HostedSupport::Unavailable {
                config.web_fetch = ResolvedWebFetchConfig::Disabled;
            }
        }
        if config.web_search.is_provider() && report.search == HostedSupport::Unavailable {
            config.web_search = ResolvedWebSearchConfig::Disabled;
        }
    }

    pub(super) fn enqueue_hosted_web_checks(
        self: &Arc<Self>,
        session_id: devo_protocol::native::ids::SessionId,
    ) {
        let runtime = Arc::downgrade(self);
        tokio::spawn(async move {
            let Some(runtime) = runtime.upgrade() else {
                return;
            };
            let Some(handle) = runtime.session(session_id).await else {
                return;
            };
            let (Some(summary), Some(context)) =
                (handle.summary().await, handle.runtime_context().await)
            else {
                return;
            };
            if let Ok(config) = context.resolve_session_turn_config(
                &summary,
                summary.model_selection(),
                summary.settings.reasoning_effort.clone(),
            ) {
                runtime.check_hosted_web(&context, &config);
            }
        });
    }
}
