//! Strict, local-only model selection for kernel children. No completion probes.
use super::*;
use devo_core::{AuthCredentialKind, ProviderConfigEntry, ProviderModelConfig, UserAuthConfigFile};
use devo_protocol::ProviderWireApi;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ChildModelEntry {
    pub(crate) provider: String,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) selector: String,
    pub(crate) thinking_levels: Vec<String>,
}

/// Inputs associated with the installed adapters, never serialized or logged.
#[derive(Clone)]
pub(super) struct ChildRouteInputs {
    providers: ProviderConfigFile,
    http: ProviderHttpConfig,
    pub(super) auth: Option<UserAuthConfigFile>,
}

impl ChildRouteInputs {
    pub(super) fn capture(config: &AppConfig, home: &Path) -> Self {
        Self {
            providers: config.provider_catalog_config(),
            http: config.provider_http.clone(),
            auth: read_user_auth_config(&home.join(AUTH_CONFIG_FILE_NAME)).ok(),
        }
    }

    fn matches(&self, current: &ChildModelCatalog, provider_id: &str, model_id: &str) -> bool {
        let Some(previous) = self.providers.providers.get(provider_id) else {
            return false;
        };
        let Some(provider) = current.configured.providers.get(provider_id) else {
            return false;
        };
        let Some(auth) = self.auth.as_ref() else {
            return false;
        };
        let previous_model = previous.models.get(model_id);
        let model = provider.models.get(model_id);
        self.http == current.config.provider_http
            && previous.base_url == provider.base_url
            && previous.wire_api == provider.wire_api
            && previous.credential == provider.credential
            && previous.headers == provider.headers
            && previous.model_overrides == provider.model_overrides
            && previous_model.and_then(|model| model.base_url.as_ref())
                == model.and_then(|model| model.base_url.as_ref())
            && previous_model.and_then(|model| model.wire_api)
                == model.and_then(|model| model.wire_api)
            && bound_credential(provider_id, previous, auth)
                == bound_credential(provider_id, provider, &current.auth)
    }
}

struct ChildModelCatalog {
    config: AppConfig,
    home: PathBuf,
    configured: ProviderConfigFile,
    effective: ProviderConfigFile,
    catalog: PresetModelCatalog,
    auth: UserAuthConfigFile,
    router: Arc<dyn ProviderRouter>,
    route_inputs: ChildRouteInputs,
}

impl SessionRuntimeContext {
    fn child_model_catalog(&self) -> anyhow::Result<ChildModelCatalog> {
        let (config, home) = {
            let mut store = self.config_store.lock().expect("config store mutex");
            store
                .reload()
                .map_err(|_| anyhow::anyhow!("cannot reload child model configuration"))?;
            (
                store.effective_config().clone(),
                store.user_config_dir().to_path_buf(),
            )
        };
        let configured = config.provider_catalog_config();
        let mut effective =
            devo_core::effective_provider_catalog_with_home(&configured, Some(&home))
                .map_err(|_| anyhow::anyhow!("cannot load configured child models"))?;
        // A user-pinned primary/small model is itself an explicit declaration,
        // even when the directory has no cached metadata for that model. Match
        // PresetModelCatalog's exact-model-before-variant interpretation.
        for selector in [
            configured.model.as_deref(),
            configured.small_model.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            let Ok((provider_id, requested_id)) = parse_selector(selector) else {
                continue;
            };
            if !configured.providers.contains_key(provider_id) {
                continue;
            }
            let Some(provider) = effective.providers.get_mut(provider_id) else {
                continue;
            };
            if provider.models.contains_key(requested_id)
                || requested_id.rsplit_once('/').is_some_and(|(id, variant)| {
                    provider
                        .models
                        .get(id)
                        .is_some_and(|model| model.variants.contains_key(variant))
                })
            {
                continue;
            }
            provider.models.entry(requested_id.to_string()).or_default();
        }
        effective.apply_model_overrides(&config.provider.model_overrides);
        let catalog = PresetModelCatalog::load_from_provider_config_with_overrides(
            &effective,
            &config.provider.model_overrides,
        )
        .map_err(|_| anyhow::anyhow!("cannot load child model capabilities"))?;
        let auth = read_user_auth_config(&home.join(AUTH_CONFIG_FILE_NAME))
            .map_err(|_| anyhow::anyhow!("cannot read provider authentication configuration"))?;
        // Take both snapshots under the router lock, matching the reload writer.
        let router = self.provider_router.lock().expect("provider router mutex");
        let route_inputs = self
            .child_route_inputs
            .lock()
            .expect("child route inputs mutex")
            .clone();
        Ok(ChildModelCatalog {
            config,
            home,
            configured,
            effective,
            catalog,
            auth,
            router: Arc::clone(&router),
            route_inputs,
        })
    }

    /// Returns only configured, enabled, currently routable models, never secrets.
    pub(crate) fn find_child_models(
        &self,
        query: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<ChildModelEntry>> {
        let snapshot = self.child_model_catalog()?;
        let query = query.unwrap_or_default().trim().to_lowercase();
        let mut entries = Vec::new();
        for model in snapshot.catalog.list_visible() {
            if snapshot.resolve(&model.slug).is_err() {
                continue;
            }
            let Some((provider, id)) = model.slug.split_once('/') else {
                continue;
            };
            if !query.is_empty()
                && !format!("{} {}", model.slug, model.display_name)
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            entries.push(ChildModelEntry {
                provider: provider.to_string(),
                id: id.to_string(),
                name: model.display_name.clone(),
                selector: model.slug.clone(),
                thinking_levels: thinking_levels(model),
            });
        }
        entries.sort_by(|a, b| a.selector.cmp(&b.selector));
        entries.truncate(limit.min(100));
        Ok(entries)
    }

    /// Resolves exact selectors, preserving transport/request metadata. Omitted
    /// values inherit the immediate parent, never the global session default.
    pub(crate) fn resolve_child_model(
        &self,
        requested: Option<&str>,
        parent_model: &str,
        thinking: Option<&str>,
        parent_thinking: Option<&str>,
    ) -> anyhow::Result<(Model, TurnConfig)> {
        let snapshot = self.child_model_catalog()?;
        let selector = requested.unwrap_or(parent_model);
        // Preserve injected single-provider runtimes only when inheriting their
        // exact current parent model. Explicit model choices never use this path.
        if requested.is_none()
            && snapshot.configured.providers.is_empty()
            && selector == self.default_model
            && !selector.contains('/')
        {
            anyhow::ensure!(
                self.provider.is_available(),
                "parent provider is unavailable"
            );
            let model = self.resolve_turn_model(Some(selector));
            anyhow::ensure!(model.slug == selector, "parent model is unavailable");
            let effort = strict_thinking(&model, thinking.or(parent_thinking))?;
            return Ok((model.clone(), TurnConfig::new(model, effort)));
        }
        let (selector, inherited_variant) =
            if requested.is_none() && snapshot.resolve(selector).is_err() {
                if let Some((base, variant)) = selector.rsplit_once('/')
                    && let Ok((provider, id, _)) = snapshot.resolve(base)
                    && snapshot.effective.providers[provider].models[id]
                        .variants
                        .get(variant)
                        .is_some_and(|variant| !variant.disabled)
                {
                    (base, Some(variant))
                } else {
                    (selector, None)
                }
            } else {
                (selector, None)
            };
        let (provider_id, model_id, route) = snapshot.resolve(selector)?;
        let model = snapshot
            .catalog
            .get(selector)
            .context("configured child model has no capability metadata")?
            .clone();
        let effort = strict_thinking(&model, thinking.or(parent_thinking))?;
        let provider = &snapshot.effective.providers[provider_id];
        let mut model_config = provider.models[model_id].clone();
        model_config.migrate_reasoning_implementation_into_variants();
        let variant = model_config.resolve_turn_variant_id(inherited_variant, effort.as_deref());
        let (defaults, headers) = provider_request_config(
            &snapshot.effective,
            provider_id,
            model_id,
            variant.as_deref(),
        );
        let request_models = provider
            .models
            .iter()
            .map(|(id, config)| {
                (
                    format!("{provider_id}/{id}"),
                    config.request_model.clone().unwrap_or_else(|| id.clone()),
                )
            })
            .collect();
        let mut turn = TurnConfig::with_provider_route_and_web_tools(
            model.clone(),
            model_config
                .request_model
                .clone()
                .unwrap_or_else(|| model_id.to_string()),
            ProviderRequestModelMap::new(request_models).with_request_config(defaults, headers),
            route,
            self.resolve_turn_web_search(
                &snapshot.config,
                &snapshot.home,
                provider.web_search.as_ref(),
                model_config.web_search.as_ref(),
            ),
            self.resolve_turn_web_fetch(
                &snapshot.config,
                provider.web_fetch.as_ref(),
                model_config.web_fetch.as_ref(),
            ),
            effort,
        );
        turn.model_binding_id = Some(selector.to_string());
        turn.variant = variant;
        Ok((model, turn))
    }
}

impl ChildModelCatalog {
    fn resolve<'a>(&self, selector: &'a str) -> anyhow::Result<(&'a str, &'a str, ProviderRoute)> {
        let (provider_id, model_id) = parse_selector(selector)?;
        let configured = self
            .configured
            .providers
            .get(provider_id)
            .context("child provider is not configured")?;
        anyhow::ensure!(
            configured.enabled != Some(false),
            "child provider is disabled"
        );
        let provider = self
            .effective
            .providers
            .get(provider_id)
            .context("child provider is unavailable")?;
        anyhow::ensure!(
            provider.enabled != Some(false),
            "child provider is disabled"
        );
        let model = provider
            .models
            .get(model_id)
            .context("child model is not configured; use rlm.find_models()")?;
        anyhow::ensure!(model.enabled != Some(false), "child model is disabled");
        let wire_api = model
            .wire_api
            .or(provider.wire_api)
            .context("child provider has no configured protocol")?;
        anyhow::ensure!(
            model
                .base_url
                .as_ref()
                .or(provider.base_url.as_ref())
                .is_some_and(|url| !url.trim().is_empty()),
            "child provider has no configured endpoint"
        );
        anyhow::ensure!(
            credentials_available(provider_id, provider, wire_api, &self.auth),
            "child provider credentials are missing, empty, or expired"
        );
        anyhow::ensure!(
            self.route_inputs.matches(self, provider_id, model_id),
            "child provider configuration changed; reconnect the provider before selecting this model"
        );
        let route = model_route(provider_id, model_id, model, wire_api);
        anyhow::ensure!(
            self.router.has_available_route(&route),
            "child model route is unavailable; reconnect the provider"
        );
        Ok((provider_id, model_id, route))
    }
}

fn parse_selector(selector: &str) -> anyhow::Result<(&str, &str)> {
    let (provider, id) = selector
        .split_once('/')
        .context("child model must use an exact provider/model selector")?;
    anyhow::ensure!(
        !provider.is_empty()
            && !id.is_empty()
            && !selector
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
            && !id.split('/').any(str::is_empty),
        "invalid child model selector"
    );
    Ok((provider, id))
}

fn model_route(
    provider: &str,
    id: &str,
    model: &ProviderModelConfig,
    wire_api: ProviderWireApi,
) -> ProviderRoute {
    if model.base_url.is_some() {
        ProviderRoute::connection_model(provider, wire_api, id)
    } else {
        ProviderRoute::connection(provider, wire_api)
    }
}

fn bound_credential<'a>(
    provider_id: &str,
    provider: &ProviderConfigEntry,
    auth: &'a UserAuthConfigFile,
) -> Option<&'a devo_core::AuthCredentialConfig> {
    if let Some(id) = provider
        .credential
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return auth.credentials.get(id).or_else(|| {
            // Only the documented legacy-id migration is allowed for explicit bindings.
            auth.credentials
                .get(&devo_core::provider_id_from_credential_id(id))
        });
    }
    crate::provider_config::resolve_provider_credential_id(provider_id, provider, auth)
        .and_then(|id| auth.credentials.get(&id))
}

fn credentials_available(
    provider_id: &str,
    provider: &ProviderConfigEntry,
    wire_api: ProviderWireApi,
    auth: &UserAuthConfigFile,
) -> bool {
    if let Some(credential) = bound_credential(provider_id, provider, auth) {
        return match credential.kind {
            AuthCredentialKind::ApiKey => !credential.value.trim().is_empty(),
            AuthCredentialKind::Oauth => {
                credential
                    .access
                    .as_deref()
                    .is_some_and(|access| !access.trim().is_empty())
                    && credential
                        .expires_at
                        .is_none_or(|expires| expires > chrono::Utc::now().timestamp())
            }
        };
    }
    // Match the adapters: OpenAI-compatible endpoints can be credentialless
    // (e.g. local servers). An explicit binding must actually resolve.
    provider
        .credential
        .as_deref()
        .is_none_or(|id| id.trim().is_empty())
        && matches!(
            wire_api,
            ProviderWireApi::OpenAIChatCompletions | ProviderWireApi::OpenAIResponses
        )
}

fn thinking_levels(model: &Model) -> Vec<String> {
    devo_protocol::resolve_thinking_fields_for_model_info(
        &model.reasoning_capability,
        model.reasoning,
        model.thinking_level_map.as_ref(),
    )
    .2
}

fn strict_thinking(model: &Model, requested: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(requested) = requested else {
        return Ok(model.normalize_reasoning_effort_selection(None));
    };
    let available = thinking_levels(model);
    anyhow::ensure!(
        available.iter().any(|value| value == requested),
        "thinking level `{requested}` is unsupported for child model `{}`; supported levels: {}",
        model.slug,
        available.join(", ")
    );
    Ok(Some(requested.to_string()))
}

#[cfg(test)]
mod tests;
