//! Hosted web capability checks are advisory and never select a fallback backend.

#[cfg(test)]
#[path = "web_status_tests.rs"]
mod tests;

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use devo_core::{ResolvedWebFetchConfig, ResolvedWebSearchConfig, TurnConfig};
use devo_protocol::native::rpc_session::{
    SessionWebStatusReadParams, SessionWebStatusReadResult, WebBackendStatus,
};
use devo_protocol::{
    HostedToolDefinition, HostedWebFetchTool, HostedWebSearchTool, ModelProfileKey, ModelRequest,
    ProviderWireApi, RequestContent, RequestMessage, ResponseContent,
};
use devo_provider::error::ProviderError;
use devo_provider::{ProviderRoute, ProviderRouter};
use lru::LruCache;
use tokio::sync::Semaphore;

use crate::SuccessResponse;
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
    ) -> Option<ProbeResult> {
        let ProviderRoute::Connection { wire_api, .. } = &config.provider_route else {
            // There is no explicit endpoint/credential binding for the bootstrap route.
            return None;
        };
        let wire = *wire_api;
        if !Capability::can_verify(wire) {
            return None;
        }
        if !router.has_available_route(&config.provider_route) {
            return None;
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
            return entry.result;
        }
        if matches!(policy, ProbePolicy::CacheOnly) {
            return None;
        }
        // Do not accumulate background requests if users switch many models quickly.
        let slot = Arc::clone(&self.slots).try_acquire_owned().ok()?;
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
        None
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

fn hosted_status_note(
    wire: ProviderWireApi,
    capability: Capability,
    result: Option<ProbeResult>,
) -> Option<String> {
    match result {
        Some(ProbeResult::Supported) => None,
        Some(ProbeResult::Rejected) => Some("Not supported or enabled on this connection".into()),
        Some(ProbeResult::Inconclusive) => {
            Some("Couldn’t verify hosted support on this connection".into())
        }
        None if wire == ProviderWireApi::GoogleGenerativeAi
            || (wire == ProviderWireApi::OpenAIChatCompletions
                && capability == Capability::Search) =>
        {
            Some("Devo does not offer this hosted tool on this protocol".into())
        }
        None if !Capability::can_verify(wire) => {
            Some("Devo can’t verify hosted support on this protocol".into())
        }
        None => None,
    }
}

impl ServerRuntime {
    pub(super) fn check_hosted_web(&self, context: &SessionRuntimeContext, config: &TurnConfig) {
        let router = context.provider_router();
        if !matches!(config.web_search, ResolvedWebSearchConfig::Disabled) {
            self.hosted_web_checks.inspect(
                Arc::clone(&router),
                config,
                Capability::Search,
                ProbePolicy::ScheduleIfNeeded,
            );
        }
        if !matches!(config.web_fetch, ResolvedWebFetchConfig::Disabled) {
            self.hosted_web_checks.inspect(
                router,
                config,
                Capability::Fetch,
                ProbePolicy::ScheduleIfNeeded,
            );
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

    pub(super) async fn handle_native_session_web_status_read(
        self: &Arc<Self>,
        id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionWebStatusReadParams =
            match self.parse_rpc_params(id.clone(), params, "session/web/status/read") {
                Ok(params) => params,
                Err(error) => return error,
            };
        let Some(handle) = self.session(params.session_id).await else {
            return self.session_missing(id);
        };
        let (Some(summary), Some(context)) =
            (handle.summary().await, handle.runtime_context().await)
        else {
            return self.session_missing(id);
        };
        let config = match context.resolve_session_turn_config(
            &summary,
            summary.model_selection(),
            summary.settings.reasoning_effort.clone(),
        ) {
            Ok(config) => config,
            Err(error) => {
                return self.error_response(
                    id,
                    crate::ProtocolErrorCode::InvalidParams,
                    error.to_string(),
                );
            }
        };
        let router = context.provider_router();
        let wire = match &config.provider_route {
            ProviderRoute::Connection { wire_api, .. } => *wire_api,
            ProviderRoute::Default => config.model.provider,
        };
        let hosted = |capability| WebBackendStatus {
            backend: "Model provider".into(),
            note: hosted_status_note(
                wire,
                capability,
                self.hosted_web_checks.inspect(
                    Arc::clone(&router),
                    &config,
                    capability,
                    ProbePolicy::CacheOnly,
                ),
            ),
        };
        use devo_protocol::native::session::WebSearchChoice;
        let search_choice = summary
            .settings
            .web_search
            .clone()
            .unwrap_or(WebSearchChoice::Inherit);
        let statuses = context.mcp_manager.statuses().await.unwrap_or_default();
        let hosted_search = hosted(Capability::Search);
        let search_choices = super::web_search_mcp::picker_entries(
            &context,
            &statuses,
            &search_choice,
            hosted_search.note.clone(),
        );
        let search = match &config.web_search {
            ResolvedWebSearchConfig::Provider => hosted_search,
            ResolvedWebSearchConfig::Mcp {
                server_id,
                tool_name,
            } => {
                let entry = super::web_search_mcp::mcp_picker_entry(
                    &context, &statuses, server_id, tool_name,
                );
                WebBackendStatus {
                    backend: entry.label,
                    note: entry.note,
                }
            }
            ResolvedWebSearchConfig::Local(local) => WebBackendStatus {
                backend: local.provider_id.clone(),
                note: None,
            },
            ResolvedWebSearchConfig::Disabled => WebBackendStatus {
                backend: "Off".into(),
                note: None,
            },
        };
        let fetch = match config.web_fetch {
            ResolvedWebFetchConfig::Provider => hosted(Capability::Fetch),
            ResolvedWebFetchConfig::Mcp { provider } => WebBackendStatus {
                backend: super::web_search_builtin::label(provider).into(),
                note: super::web_search_builtin::status_note(&context, provider),
            },
            ResolvedWebFetchConfig::Local => WebBackendStatus {
                backend: "Direct HTTP".into(),
                note: None,
            },
            ResolvedWebFetchConfig::Disabled => WebBackendStatus {
                backend: "Off".into(),
                note: None,
            },
        };
        serde_json::to_value(SuccessResponse {
            id,
            result: SessionWebStatusReadResult {
                search,
                fetch,
                search_choice,
                search_choices,
            },
        })
        .expect("serialize session web status")
    }
}
