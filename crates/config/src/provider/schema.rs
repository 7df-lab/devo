use std::collections::BTreeMap;

use devo_protocol::ProviderWireApi;
use serde::Deserialize;
use serde::Serialize;

use crate::WebFetchConfig;
use crate::WebSearchConfig;

pub(crate) const AUTH_CONFIG_VERSION: u32 = 1;

/// The preferred authentication method for the active provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreferredAuthMethod {
    /// Use an API key or bearer token.
    Apikey,
}

impl<'de> Deserialize<'de> for PreferredAuthMethod {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "apikey" | "api_key" => Ok(Self::Apikey),
            other => Err(serde::de::Error::custom(format!(
                "unsupported preferred_auth_method `{other}`"
            ))),
        }
    }
}

/// Legacy model entry stored under old `[model_providers]` config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfiguredModel {
    /// The model slug or custom model name.
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// One persisted provider vendor record stored under `[providers.<id>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderVendorConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Credential id in user-scoped `auth.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    /// Raw JSON object string containing provider-specific HTTP headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wire_apis: Vec<ProviderWireApi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_fetch: Option<WebFetchConfig>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl ProviderVendorConfig {
    /// Returns whether the profile has no configured values.
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.base_url.is_none()
            && self.credential.is_none()
            && self.headers.is_none()
            && self.wire_apis.is_empty()
            && self.web_search.is_none()
            && self.web_fetch.is_none()
            && self.enabled
    }
}

/// Backward-compatible public name for provider vendor config.
pub type ModelProviderConfig = ProviderVendorConfig;

/// One invocable model binding stored under `[model_bindings.<id>]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelBindingConfig {
    pub model_slug: String,
    pub provider: String,
    pub model_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default = "default_provider_wire_api")]
    pub invocation_method: ProviderWireApi,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_fetch: Option<WebFetchConfig>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for ModelBindingConfig {
    fn default() -> Self {
        Self {
            model_slug: String::new(),
            provider: String::new(),
            model_name: String::new(),
            display_name: None,
            invocation_method: default_provider_wire_api(),
            default_reasoning_effort: None,
            web_search: None,
            web_fetch: None,
            enabled: true,
        }
    }
}

/// Durable default selections stored under `[defaults]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDefaultsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<String>,
}

/// Legacy provider profile stored under old `[model_providers.<id>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyModelProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<ProviderWireApi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ConfiguredModel>,
}

/// User-scoped credential file stored at `<user-config-dir>/auth.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAuthConfigFile {
    #[serde(default = "default_auth_config_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub credentials: BTreeMap<String, AuthCredentialConfig>,
}

impl Default for UserAuthConfigFile {
    fn default() -> Self {
        Self {
            version: AUTH_CONFIG_VERSION,
            credentials: BTreeMap::new(),
        }
    }
}

/// One secret value in user-scoped auth storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthCredentialConfig {
    pub kind: AuthCredentialKind,
    pub value: String,
}

/// Supported credential kinds in `auth.json`.
/// TODO: support oauth in the near future.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthCredentialKind {
    ApiKey,
}

/// Provider-owned portion of app config, including active model selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfigSection {
    #[serde(default, skip_serializing_if = "ProviderDefaultsConfig::is_empty")]
    pub defaults: ProviderDefaultsConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Logical thinking selection for the active model, such as `disabled`,
    /// `enabled`, or one effort-like level supported by the selected model.
    ///
    /// This stores the user-facing selection, not a provider-specific request
    /// field. The runtime later resolves it into the final request model,
    /// request `thinking` parameter, effective reasoning effort, and any
    /// provider-specific extra payload.
    #[serde(alias = "model_thinking")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_thinking_selection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_auto_compact_token_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_context_window: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_response_storage: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_auth_method: Option<PreferredAuthMethod>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderVendorConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_bindings: BTreeMap<String, ModelBindingConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_providers: BTreeMap<String, LegacyModelProviderConfig>,
}

impl ProviderConfigSection {
    pub(crate) fn merge_overlay(&mut self, overlay: Self) {
        if overlay.model_provider.is_some() {
            self.model_provider = overlay.model_provider;
        }
        if overlay.model.is_some() {
            self.model = overlay.model;
        }
        if overlay.model_thinking_selection.is_some() {
            self.model_thinking_selection = overlay.model_thinking_selection;
        }
        if overlay.model_auto_compact_token_limit.is_some() {
            self.model_auto_compact_token_limit = overlay.model_auto_compact_token_limit;
        }
        if overlay.model_context_window.is_some() {
            self.model_context_window = overlay.model_context_window;
        }
        if overlay.disable_response_storage.is_some() {
            self.disable_response_storage = overlay.disable_response_storage;
        }
        if overlay.preferred_auth_method.is_some() {
            self.preferred_auth_method = overlay.preferred_auth_method;
        }
        if overlay.defaults.model_binding.is_some() {
            self.defaults.model_binding = overlay.defaults.model_binding;
        }
        for (provider_id, overlay_provider) in overlay.providers {
            let provider = self.providers.entry(provider_id).or_default();
            if !overlay_provider.name.is_empty() {
                provider.name = overlay_provider.name;
            }
            if overlay_provider.base_url.is_some() {
                provider.base_url = overlay_provider.base_url;
            }
            if overlay_provider.credential.is_some() {
                provider.credential = overlay_provider.credential;
            }
            if overlay_provider.headers.is_some() {
                provider.headers = overlay_provider.headers;
            }
            if !overlay_provider.wire_apis.is_empty() {
                provider.wire_apis = overlay_provider.wire_apis;
            }
            if overlay_provider.web_search.is_some() {
                provider.web_search = overlay_provider.web_search;
            }
            if overlay_provider.web_fetch.is_some() {
                provider.web_fetch = overlay_provider.web_fetch;
            }
            provider.enabled = overlay_provider.enabled;
        }
        for (binding_id, overlay_binding) in overlay.model_bindings {
            let binding = self.model_bindings.entry(binding_id).or_default();
            if !overlay_binding.model_slug.is_empty() {
                binding.model_slug = overlay_binding.model_slug;
            }
            if !overlay_binding.provider.is_empty() {
                binding.provider = overlay_binding.provider;
            }
            if !overlay_binding.model_name.is_empty() {
                binding.model_name = overlay_binding.model_name;
            }
            if overlay_binding.display_name.is_some() {
                binding.display_name = overlay_binding.display_name;
            }
            binding.invocation_method = overlay_binding.invocation_method;
            if overlay_binding.default_reasoning_effort.is_some() {
                binding.default_reasoning_effort = overlay_binding.default_reasoning_effort;
            }
            if overlay_binding.web_search.is_some() {
                binding.web_search = overlay_binding.web_search;
            }
            if overlay_binding.web_fetch.is_some() {
                binding.web_fetch = overlay_binding.web_fetch;
            }
            binding.enabled = overlay_binding.enabled;
        }
    }
}

impl ProviderDefaultsConfig {
    pub fn is_empty(&self) -> bool {
        self.model_binding.is_none()
    }
}

/// Provider HTTP settings shared by model-provider requests.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderHttpConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

impl ProviderHttpConfig {
    pub fn is_empty(&self) -> bool {
        self.proxy_url.is_none()
    }
}

/// The fully-resolved provider settings that can be forwarded to a server process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProviderSettings {
    /// Selected provider identifier from `[providers.<id>]`.
    pub provider_id: String,
    /// Selected provider transport implementation.
    pub wire_api: ProviderWireApi,
    /// Final model identifier.
    pub model: String,
    /// Optional provider base URL override.
    pub base_url: Option<String>,
    /// Optional provider API key override.
    pub api_key: Option<String>,
    /// Optional global provider HTTP proxy URL.
    pub proxy_url: Option<String>,
    /// Optional raw provider custom header JSON object string.
    pub headers: Option<String>,
    /// Optional active model auto-compaction threshold in tokens.
    pub model_auto_compact_token_limit: Option<u32>,
    /// Optional active model context window override in tokens.
    pub model_context_window: Option<u32>,
    /// Optional logical thinking selection for the active model.
    pub model_thinking_selection: Option<String>,
    /// Whether provider-side response storage should be disabled.
    pub disable_response_storage: bool,
    /// Preferred authentication method for the active provider.
    pub preferred_auth_method: Option<PreferredAuthMethod>,
}

fn default_true() -> bool {
    true
}

fn default_auth_config_version() -> u32 {
    AUTH_CONFIG_VERSION
}

fn default_provider_wire_api() -> ProviderWireApi {
    ProviderWireApi::OpenAIChatCompletions
}
