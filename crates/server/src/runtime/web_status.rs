//! Hosted web capability checks are advisory and never select a fallback backend.

#[cfg(test)]
#[path = "web_status_tests.rs"]
mod tests;

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use devo_core::{ResolvedWebFetchConfig, ResolvedWebSearchConfig, TurnConfig};
#[path = "web_picker_status.rs"]
mod picker;
use devo_protocol::{
    HostedToolDefinition, HostedWebFetchTool, HostedWebSearchTool, ModelProfileKey, ModelRequest,
    ProviderWireApi, RequestContent, RequestMessage, ResponseContent,
};
use devo_provider::error::ProviderError;
use devo_provider::{ProviderRoute, ProviderRouter};
use lru::LruCache;
use tokio::sync::Semaphore;

use crate::runtime::ServerRuntime;
use crate::session_context::SessionRuntimeContext;

const PROBE_TIMEOUT: Duration = Duration::from_secs(12);
const RESULT_TTL: Duration = Duration::from_secs(3600);
const INCONCLUSIVE_TTL: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Capability {
    Search,
    Fetch,
}

impl Capability {
    fn name(self) -> &'static str {
        match self {
            Self::Search => "web_search",
            Self::Fetch => "web_fetch",
        }
    }

    fn can_verify(wire: ProviderWireApi) -> bool {
        matches!(
            wire,
            ProviderWireApi::AnthropicMessages | ProviderWireApi::OpenAIResponses
        )
    }

    fn definition(self) -> HostedToolDefinition {
        match self {
            Self::Search => HostedToolDefinition::WebSearch(HostedWebSearchTool {
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

    fn prompt(self) -> &'static str {
        match self {
            Self::Search => {
                "Use hosted web search once for the exact public query ‘IANA example domains’. Then reply OK."
            }
            Self::Fetch => {
                "Use the standalone hosted web fetch tool once to fetch https://example.com/. Then reply OK."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeResult {
    Supported,
    Rejected,
    Inconclusive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostedSupport {
    Supported,
    Unavailable,
    Checking,
    Unverified,
}

#[derive(Clone, Copy)]
enum ProbePolicy {
    ScheduleIfNeeded,
    CacheOnly,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ProbeKey {
    router_id: usize,
    route: ProviderRoute,
    model_slug: String,
    request_model: String,
    variant: Option<String>,
    capability: Capability,
}

// Cache by router snapshot: a reload can change the endpoint or credentials even
// when the public connection ID is unchanged. Hold only a weak reference so the
// pointer cannot be reused for another router while its key is in the cache.
struct Entry {
    _router: Weak<dyn ProviderRouter>,
    expires_at: Instant,
    result: Option<ProbeResult>,
}

#[derive(Clone)]
pub(super) struct HostedWebChecks {
    entries: Arc<Mutex<LruCache<ProbeKey, Entry>>>,
    slots: Arc<Semaphore>,
}

impl Default for HostedWebChecks {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(128).unwrap()))),
            slots: Arc::new(Semaphore::new(4)),
        }
    }
}

impl HostedWebChecks {
    fn inspect(
        &self,
        router: Arc<dyn ProviderRouter>,
        config: &TurnConfig,
        capability: Capability,
        policy: ProbePolicy,
    ) -> HostedSupport {
        let wire = match &config.provider_route {
            ProviderRoute::Connection { wire_api, .. } => *wire_api,
            ProviderRoute::Default => config.model.provider,
        };
        if !Capability::can_verify(wire) {
            return HostedSupport::Unavailable;
        }
        if matches!(config.provider_route, ProviderRoute::Default)
            || !router.has_available_route(&config.provider_route)
        {
            // No explicit endpoint or usable credential binding for this route.
            return HostedSupport::Unverified;
        }
        let reasoning = config
            .model
            .resolve_reasoning_effort_selection(config.reasoning_effort_selection.as_deref());
        let key = ProbeKey {
            router_id: Arc::as_ptr(&router).cast::<()>() as usize,
            route: config.provider_route.clone(),
            model_slug: reasoning.request_model.clone(),
            request_model: config.provider_request_model(&reasoning.request_model),
            variant: config.variant.clone(),
            capability,
        };
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("hosted web probe cache mutex");
        if let Some(entry) = entries.get(&key)
            && entry.expires_at > now
        {
            return match entry.result {
                Some(ProbeResult::Supported) => HostedSupport::Supported,
                Some(ProbeResult::Rejected) => HostedSupport::Unavailable,
                Some(ProbeResult::Inconclusive) => HostedSupport::Unverified,
                None => HostedSupport::Checking,
            };
        }
        if matches!(policy, ProbePolicy::CacheOnly) {
            return HostedSupport::Unverified;
        }
        // Do not accumulate background requests if users switch many models quickly.
        let Ok(slot) = Arc::clone(&self.slots).try_acquire_owned() else {
            return HostedSupport::Unverified;
        };
        entries.put(
            key.clone(),
            Entry {
                _router: Arc::downgrade(&router),
                expires_at: now + PROBE_TIMEOUT + Duration::from_secs(1),
                result: None,
            },
        );
        drop(entries);

        // Only pass model-routing inputs, never history, tools, workspace context,
        // or arbitrary body overrides (which can replace the bounded probe input).
        let request = ModelRequest {
            model_slug: ModelProfileKey::CatalogSlug(key.model_slug.clone()),
            model: key.request_model.clone(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".into(),
                content: vec![RequestContent::Text {
                    text: capability.prompt().into(),
                }],
            }],
            max_tokens: 256,
            tools: None,
            hosted_tools: vec![capability.definition()],
            sampling: Default::default(),
            request_thinking: reasoning.request_thinking,
            reasoning_effort: reasoning.request_reasoning_effort,
            extra_body: devo_core::add_model_request_headers(
                None,
                config.provider_request_models.request_headers(),
            ),
        };
        let entries = Arc::clone(&self.entries);
        tokio::spawn(async move {
            let _slot = slot;
            let result = match tokio::time::timeout(
                PROBE_TIMEOUT,
                router.complete(key.route.clone(), request),
            )
            .await
            {
                Ok(Ok(response)) => classify_response(wire, capability, &response.content),
                Ok(Err(error)) if explicitly_rejected(capability, &error) => ProbeResult::Rejected,
                Ok(Err(_)) | Err(_) => ProbeResult::Inconclusive,
            };
            let mut entries = entries.lock().expect("hosted web probe cache mutex");
            if let Some(entry) = entries.get_mut(&key)
                && entry.expires_at == now + PROBE_TIMEOUT + Duration::from_secs(1)
            {
                entry.result = Some(result);
                entry.expires_at = Instant::now()
                    + match result {
                        ProbeResult::Inconclusive => INCONCLUSIVE_TTL,
                        ProbeResult::Supported | ProbeResult::Rejected => RESULT_TTL,
                    };
            }
        });
        HostedSupport::Checking
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
        if let Some(error) = output
            .as_ref()
            .and_then(|output| output.get("error_code").and_then(serde_json::Value::as_str))
        {
            if matches!(
                error,
                "not_supported" | "tool_not_supported" | "tool_not_enabled" | "permission_denied"
            ) {
                outcome = ProbeResult::Rejected;
            }
            continue;
        }
        if status.as_deref() != Some("completed") {
            continue;
        }
        // Anthropic emits a separate result payload, including on tool-level errors.
        // Responses may omit output; a completed typed search action is still evidence.
        let structured_result = output.as_ref().is_some_and(|output| {
            (capability == Capability::Search && output.is_array())
                || output.get("type").and_then(serde_json::Value::as_str)
                    == Some("web_fetch_result")
        });
        let completed_search_action = wire == ProviderWireApi::OpenAIResponses
            && capability == Capability::Search
            && (input.get("type").and_then(serde_json::Value::as_str) == Some("search")
                || input
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .is_some())
            && output
                .as_ref()
                .is_none_or(|output| output.get("error").is_none());
        if structured_result || completed_search_action {
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
    message.contains(capability.name())
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

impl ServerRuntime {
    pub(super) fn check_hosted_web(&self, context: &SessionRuntimeContext, config: &TurnConfig) {
        // Even when an MCP backend is selected, users need to know what the
        // current model can do. When web is fully off, don't start network probes.
        if matches!(config.web_search, ResolvedWebSearchConfig::Disabled)
            && matches!(config.web_fetch, ResolvedWebFetchConfig::Disabled)
        {
            return;
        }
        let router = context.provider_router();
        for capability in [Capability::Search, Capability::Fetch] {
            self.hosted_web_checks.inspect(
                Arc::clone(&router),
                config,
                capability,
                ProbePolicy::ScheduleIfNeeded,
            );
        }
    }

    pub(in crate::runtime) fn suppress_rejected_hosted_tools(
        &self,
        context: &SessionRuntimeContext,
        config: &mut TurnConfig,
    ) {
        let router = context.provider_router();
        let unavailable = |capability, config: &TurnConfig| {
            self.hosted_web_checks.inspect(
                Arc::clone(&router),
                config,
                capability,
                ProbePolicy::CacheOnly,
            ) == HostedSupport::Unavailable
        };
        // Avoid sending a known rejected tool (it can reject the whole model
        // request). Unknown and pending probes never disable a selected tool.
        if config.web_search.is_provider() && unavailable(Capability::Search, config) {
            config.web_search = ResolvedWebSearchConfig::Disabled;
        }
        if config.web_fetch.is_provider() && unavailable(Capability::Fetch, config) {
            config.web_fetch = ResolvedWebFetchConfig::Disabled;
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
