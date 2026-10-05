use anyhow::Context;
use devo_core::AUTH_CONFIG_FILE_NAME;
use devo_core::Model;
use devo_core::ModelCatalog;
use devo_core::PROVIDER_CONFIG_FILE_NAME;
use devo_core::PresetModelCatalog;
use devo_core::ProviderHttpConfig;
use devo_core::read_user_auth_config;
use devo_core::test_model_connection;
use devo_protocol::ModelProfileKey;
use devo_provider::ProviderHttpOptions;
use devo_util_paths::current_user_config_file;

use crate::ProtocolErrorCode;
use crate::SuccessResponse;

use super::ServerRuntime;

fn mask_secret(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return String::from("****");
    }
    if trimmed.len() <= 4 {
        return "*".repeat(trimmed.len().max(4));
    }
    format!("****{}", &trimmed[trimmed.len().saturating_sub(4)..])
}

/// Ensures the provider entry points at the newly written credential.
///
/// The entry is patched in whichever catalog file it actually lives in —
/// `providers.json` or `custom-providers.json`. Writing a fresh stub into
/// `providers.json` when the full Connection lives in the custom file would
/// later be "migrated" over it by [`migrate_custom_providers_file`] and
/// clobber `base_url`/`wire_api`/`models`, leaving the provider unusable.
fn bind_provider_credential_id(
    user_dir: &std::path::Path,
    provider_id: &str,
    credential_id: &str,
) -> Result<(), String> {
    let config_file = user_dir.join(PROVIDER_CONFIG_FILE_NAME);
    let custom_file = user_dir.join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME);
    let mut config =
        devo_core::read_provider_catalog_config(&config_file).map_err(|error| error.to_string())?;

    if let Some(entry) = config.providers.get_mut(provider_id) {
        entry.credential = Some(credential_id.to_string());
        if entry.enabled.is_none() {
            entry.enabled = Some(true);
        }
        return devo_core::write_provider_catalog_config(&config_file, &config)
            .map_err(|error| error.to_string());
    }

    let mut custom =
        devo_core::read_provider_catalog_config(&custom_file).map_err(|error| error.to_string())?;
    if let Some(entry) = custom.providers.get_mut(provider_id) {
        entry.credential = Some(credential_id.to_string());
        if entry.enabled.is_none() {
            entry.enabled = Some(true);
        }
        return devo_core::write_provider_catalog_config(&custom_file, &custom)
            .map_err(|error| error.to_string());
    }

    // Provider unknown to both files: seed a stub in `providers.json`;
    // `migrate_custom_providers_file` splits it out on the next start.
    config.providers.insert(
        provider_id.to_string(),
        devo_core::ProviderConfigEntry {
            name: Some(provider_id.to_string()),
            credential: Some(credential_id.to_string()),
            enabled: Some(true),
            ..Default::default()
        },
    );
    devo_core::write_provider_catalog_config(&config_file, &config)
        .map_err(|error| error.to_string())
}

impl ServerRuntime {
    /// Reloads Connection routes on the process context and every distinct
    /// live session context after a provider connection mutation.
    ///
    /// Sessions capture their provider router at creation, so without this
    /// sweep a connection added or removed through `provider/upsert` only
    /// reaches sessions created afterwards — live turns keep failing with
    /// "provider route not configured" until a server restart. Contexts are
    /// shared per workspace (and with spawned subagents), so each distinct
    /// context reloads exactly once; rebuilds may refresh OAuth credentials.
    async fn reload_live_provider_routers(&self) {
        let mut reloaded: Vec<std::sync::Arc<crate::session_context::SessionRuntimeContext>> =
            Vec::new();
        let process_context = std::sync::Arc::clone(&self.deps.process_context);
        if let Err(error) = process_context.reload_provider_router().await {
            tracing::warn!(
                error = %error,
                "failed to reload provider routes on process context"
            );
        }
        reloaded.push(process_context);
        for handle in self.list_session_handles().await {
            let Some(session_context) = handle.runtime_context().await else {
                continue;
            };
            if reloaded
                .iter()
                .any(|reloaded| std::sync::Arc::ptr_eq(reloaded, &session_context))
            {
                continue;
            }
            if let Err(error) = session_context.reload_provider_router().await {
                tracing::warn!(
                    error = %error,
                    "failed to reload provider routes on session context"
                );
            }
            reloaded.push(session_context);
        }
    }

    /// Native `provider/list` (ratified #11), backed by the bundled provider
    /// directory plus the effective config store and projected into the
    /// canonical camelCase result.
    pub(super) async fn handle_native_provider_list(
        &self,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let mut providers = self.deps.model_catalog.list_providers();
        let template_provider_ids = self.deps.model_catalog.list_template_provider_ids();
        let store = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned");
        let config = store.effective_config();
        let home = store.user_config_dir().to_path_buf();
        let live_catalog = PresetModelCatalog::load_from_provider_config_with_home(
            &config.provider_catalog,
            &config.provider.model_overrides,
            Some(home.as_path()),
        )
        .ok();
        let catalog: &dyn ModelCatalog = live_catalog
            .as_ref()
            .map(|catalog| catalog as &dyn ModelCatalog)
            .unwrap_or(self.deps.model_catalog.as_ref());
        let connected_provider_ids = match store.provider_connection_ids() {
            Ok(provider_ids) => provider_ids,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read provider connections: {error}"),
                );
            }
        };
        let connection_models = match store.provider_connection_models() {
            Ok(models) => models,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read provider Connection models: {error}"),
                );
            }
        };
        let configured_providers = match store.provider_connections() {
            Ok(providers) => providers,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read provider Connections: {error}"),
                );
            }
        };
        for configured in configured_providers {
            if let Some(directory_entry) =
                providers.iter_mut().find(|entry| entry.id == configured.id)
            {
                *directory_entry = configured;
            } else {
                providers.push(configured);
            }
        }
        let providers = providers
            .into_iter()
            .map(|provider| {
                let mut info = canonical_provider_info(provider, catalog);
                if let Some(config) = store
                    .effective_config()
                    .provider_catalog
                    .providers
                    .get(&info.id)
                {
                    info.options = config.options.clone();
                    info.request = config.request.clone();
                    if let Some(headers) = &config.headers {
                        info.headers = headers.clone();
                    }
                }
                info
            })
            .collect();
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ProviderListResult {
                providers,
                template_provider_ids,
                connected_provider_ids,
                connection_models,
            },
        })
        .expect("serialize canonical provider/list response")
    }

    /// Native `credential/list` — ids and masks only (L2-DES-AUTH-001).
    pub(super) async fn handle_native_credential_list(
        &self,
        request_id: serde_json::Value,
    ) -> serde_json::Value {
        let auth_file = match current_user_config_file() {
            Ok(path) => path.with_file_name(AUTH_CONFIG_FILE_NAME),
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("could not determine user config path: {error}"),
                );
            }
        };
        let auth = match read_user_auth_config(&auth_file) {
            Ok(auth) => auth,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read auth.json: {error}"),
                );
            }
        };
        let credentials = auth
            .credentials
            .into_iter()
            .map(|(id, credential)| {
                let (kind, masked) = match credential.kind {
                    devo_core::AuthCredentialKind::ApiKey => {
                        ("api_key".to_string(), mask_secret(&credential.value))
                    }
                    devo_core::AuthCredentialKind::Oauth => (
                        "oauth".to_string(),
                        mask_secret(credential.access.as_deref().unwrap_or("")),
                    ),
                };
                let provider = devo_core::provider_id_from_credential_id(&id);
                devo_protocol::native::rpc_admin::CredentialInfo {
                    id,
                    provider,
                    masked,
                    kind: Some(kind),
                }
            })
            .collect();
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::CredentialListResult { credentials },
        })
        .expect("serialize credential/list response")
    }

    /// Native `credential/set` — write-only secrets (L2-DES-AUTH-001).
    pub(super) async fn handle_native_credential_set(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::CredentialSetParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid credential/set params: {error}"),
                    );
                }
            };
        let provider = params.provider.trim();
        if provider.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "credential/set provider must not be empty",
            );
        }
        let kind = params
            .kind
            .as_deref()
            .unwrap_or("api_key")
            .trim()
            .to_ascii_lowercase();
        let user_dir = match current_user_config_file() {
            Ok(path) => path
                .parent()
                .map(|parent| parent.to_path_buf())
                .unwrap_or(path),
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("could not determine user config path: {error}"),
                );
            }
        };
        let (credential_id, masked, kind_label) = if kind == "oauth" {
            let access = match params
                .access
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                Some(access) => access,
                None => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "oauth credential/set requires access",
                    );
                }
            };
            let credential_id = params
                .id
                .filter(|id| !id.trim().is_empty())
                .map(|id| devo_core::provider_id_from_credential_id(&id))
                .unwrap_or_else(|| provider.to_string());
            if let Err(error) = devo_core::upsert_user_auth_oauth(
                &user_dir,
                &credential_id,
                devo_core::UpsertOauthCredential {
                    access: access.to_string(),
                    refresh: params.refresh.clone(),
                    expires_at: params.expires_at,
                    account_id: params.account_id.clone(),
                    enterprise_url: params.enterprise_url.clone(),
                },
            ) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist oauth credential: {error}"),
                );
            }
            if let Err(error) = bind_provider_credential_id(&user_dir, provider, &credential_id) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to bind provider credential: {error}"),
                );
            }
            (credential_id, mask_secret(access), "oauth".to_string())
        } else {
            let secret = match params
                .secret
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(secret) => secret,
                None => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "api_key credential/set requires secret",
                    );
                }
            };
            let credential_id = params
                .id
                .filter(|id| !id.trim().is_empty())
                .map(|id| devo_core::provider_id_from_credential_id(&id))
                .unwrap_or_else(|| devo_core::default_provider_credential_id(provider));
            if let Err(error) =
                devo_core::upsert_user_auth_api_key(&user_dir, &credential_id, secret)
            {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist api key credential: {error}"),
                );
            }
            if let Err(error) = bind_provider_credential_id(&user_dir, provider, &credential_id) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to bind provider credential: {error}"),
                );
            }
            (credential_id, mask_secret(secret), "api_key".to_string())
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::CredentialSetResult {
                credential: devo_protocol::native::rpc_admin::CredentialInfo {
                    id: credential_id,
                    provider: provider.to_string(),
                    masked,
                    kind: Some(kind_label),
                },
            },
        })
        .expect("serialize credential/set response")
    }

    /// Native `credential/delete`.
    pub(super) async fn handle_native_credential_delete(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::CredentialDeleteParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid credential/delete params: {error}"),
                    );
                }
            };
        let user_dir = match current_user_config_file() {
            Ok(path) => path
                .parent()
                .map(|parent| parent.to_path_buf())
                .unwrap_or(path),
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("could not determine user config path: {error}"),
                );
            }
        };
        match devo_core::remove_user_auth_credential(&user_dir, &params.credential_id) {
            Ok(_) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_admin::CredentialDeleteResult {},
            })
            .expect("serialize credential/delete response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to delete credential: {error}"),
            ),
        }
    }

    /// Native `provider/upsert` (ratified #11).
    pub(super) async fn handle_native_provider_upsert(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ProviderUpsertParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical provider/upsert params: {error}"),
                    );
                }
            };
        let Some(_provider_id) = normalized_provider_id(&params.provider.id)
            .or_else(|| normalized_provider_id(&params.provider.name))
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "provider name cannot be empty",
            );
        };
        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join(PROVIDER_CONFIG_FILE_NAME)
                .display()
                .to_string()
        };
        if let Some(reason) = self
            .config_change_hook_block_reason("user_settings", Some(config_file))
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::PolicyDenied,
                format!("config change blocked by hook: {reason}"),
            );
        }

        let default_model = params.default_model.clone();
        let small_model = params.small_model.clone();
        let provider_id = params.provider.id.clone();
        let builtin = devo_core::builtin_provider_config().ok();
        let provider = {
            let mut store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            let baseline = builtin
                .as_ref()
                .and_then(|config| config.providers.get(&provider_id));
            match store.upsert_provider_connection_with_baseline(
                params.provider,
                params.default_model,
                params.small_model,
                params.api_key,
                baseline,
            ) {
                Ok(provider) => provider,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        error.to_string(),
                    );
                }
            }
        };
        self.deps.invalidate_workspace_contexts();
        self.reload_live_provider_routers().await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ProviderUpsertResult {
                provider,
                default_model,
                small_model,
            },
        })
        .expect("serialize canonical provider/upsert response")
    }

    /// Native provider/disconnect removes one user Connection while leaving
    /// the built-in provider directory untouched.
    pub(super) async fn handle_native_provider_disconnect(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ProviderDisconnectParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical provider/disconnect params: {error}"),
                    );
                }
            };
        let provider_id = params.provider_id.trim();
        if provider_id.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "provider_id cannot be empty",
            );
        }
        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join(PROVIDER_CONFIG_FILE_NAME)
                .display()
                .to_string()
        };
        if let Some(reason) = self
            .config_change_hook_block_reason("user_settings", Some(config_file))
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::PolicyDenied,
                format!("config change blocked by hook: {reason}"),
            );
        }

        {
            let mut store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            if let Err(error) = store.disconnect_provider(provider_id) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    error.to_string(),
                );
            }
        }
        self.deps.invalidate_workspace_contexts();
        self.reload_live_provider_routers().await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ProviderDisconnectResult {
                provider_id: provider_id.to_string(),
            },
        })
        .expect("serialize canonical provider/disconnect response")
    }

    /// Native provider/model/remove removes one model from a user Connection
    /// while leaving the provider template and its built-in models untouched.
    pub(super) async fn handle_native_provider_model_remove(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ProviderModelRemoveParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical provider/model/remove params: {error}"),
                    );
                }
            };
        let provider_id = params.provider_id.trim();
        let model_id = params.model_id.trim();
        if provider_id.is_empty() || model_id.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "provider_id and model_id cannot be empty",
            );
        }
        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join(PROVIDER_CONFIG_FILE_NAME)
                .display()
                .to_string()
        };
        if let Some(reason) = self
            .config_change_hook_block_reason("user_settings", Some(config_file))
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::PolicyDenied,
                format!("config change blocked by hook: {reason}"),
            );
        }

        let mut store = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned");
        let connected_provider_ids = match store.provider_connection_ids() {
            Ok(provider_ids) => provider_ids,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read provider connections: {error}"),
                );
            }
        };
        if !connected_provider_ids.iter().any(|id| id == provider_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("provider {provider_id} is not a user Connection"),
            );
        }
        if let Err(error) = store.remove_provider_model(provider_id, model_id) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                error.to_string(),
            );
        }
        drop(store);
        self.deps.invalidate_workspace_contexts();

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_admin::ProviderModelRemoveResult {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
            },
        })
        .expect("serialize canonical provider/model/remove response")
    }

    /// Native `provider/validate` (ratified #11).
    pub(super) async fn handle_native_provider_validate(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_admin::ProviderValidateParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical provider/validate params: {error}"),
                    );
                }
            };
        let Some(provider_id) = normalized_provider_id(&params.provider.id)
            .or_else(|| normalized_provider_id(&params.provider.name))
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "provider name cannot be empty",
            );
        };
        if params.model.trim().is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "model cannot be empty",
            );
        }
        let _ = provider_id;
        let provider_http = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store.effective_config().provider_http.clone()
        };

        match validate_provider_candidate(params, self.deps.model_catalog.as_ref(), provider_http)
            .await
        {
            Ok(reply_preview) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_admin::ProviderValidateResult { reply_preview },
            })
            .expect("serialize canonical provider/validate response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                error.to_string(),
            ),
        }
    }
}

/// Completes a provider entry with the model metadata from the bundled
/// directory when the caller supplied only a connection and model id.
///
/// Connection overlays win on key conflicts. Disabled models remain present
/// so settings UIs can show them with `enabled=false`.
fn canonical_provider_info(
    mut provider: devo_protocol::ProviderInfo,
    catalog: &dyn ModelCatalog,
) -> devo_protocol::ProviderInfo {
    let catalog_models = catalog.list_provider_models(&provider.id);
    if provider.models.is_empty() {
        provider.models = catalog_models;
    } else if !catalog_models.is_empty() {
        let mut merged = catalog_models;
        for (model_id, model) in provider.models {
            merged.insert(model_id, model);
        }
        provider.models = merged;
    }
    provider
}
fn normalized_provider_id(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

async fn validate_provider_candidate(
    params: devo_protocol::native::rpc_admin::ProviderValidateParams,
    catalog: &dyn ModelCatalog,
    provider_http: ProviderHttpConfig,
) -> anyhow::Result<String> {
    let provider_id = normalized_provider_id(&params.provider.id)
        .or_else(|| normalized_provider_id(&params.provider.name))
        .context("provider name cannot be empty")?;
    if params.provider.wire_apis.is_empty() {
        anyhow::bail!("wire_apis must contain at least one wire API");
    }

    let requested_model_id = params
        .model
        .strip_prefix(&format!("{provider_id}/"))
        .unwrap_or(&params.model);
    let (model_id, model_info) = if let Some(model) = params.provider.models.get(requested_model_id)
    {
        (requested_model_id.to_string(), model)
    } else if let Some((base_model_id, variant_id)) = requested_model_id.rsplit_once('/') {
        let model = params
            .provider
            .models
            .get(base_model_id)
            .filter(|model| model.variants.contains_key(variant_id))
            .context("model variant is not present in provider directory")?;
        (base_model_id.to_string(), model)
    } else {
        anyhow::bail!(
            "model {} is not present in provider directory",
            params.model
        );
    };
    let wire_api = model_info
        .wire_api
        .or_else(|| params.provider.wire_apis.first().cloned())
        .context("provider has no usable wire API")?;
    if !params.provider.wire_apis.contains(&wire_api) {
        anyhow::bail!("model wire API must be supported by provider");
    }

    let model_ref = format!("{provider_id}/{model_id}");
    let (validation_model, model_profile) = resolve_validation_model(catalog, wire_api, &model_ref);
    let api_key = resolve_validation_api_key(&provider_id, &params).await?;
    let headers = (!params.provider.headers.is_empty())
        .then(|| serde_json::to_string(&params.provider.headers))
        .transpose()?;
    let provider = crate::provider_config::build_provider_adapter(
        wire_api,
        params.provider.base_url.clone(),
        api_key,
        ProviderHttpOptions::from_raw_with_no_proxy(
            provider_http.proxy_url,
            provider_http.no_proxy,
            headers,
        )?,
        None,
    )?;

    test_model_connection(
        provider.as_ref(),
        &validation_model,
        model_profile,
        requested_model_id,
        "Reply with OK only.",
    )
    .await
    .map_err(Into::into)
}
fn resolve_validation_model(
    catalog: &dyn ModelCatalog,
    wire_api: devo_core::ProviderWireApi,
    model_slug: &str,
) -> (Model, ModelProfileKey) {
    if let Some(entry) = catalog.get(model_slug) {
        let mut model = entry.clone();
        model.provider = wire_api;
        return (model, ModelProfileKey::CatalogSlug(model_slug.to_string()));
    }
    (
        Model {
            slug: model_slug.to_string(),
            display_name: model_slug.to_string(),
            provider: wire_api,
            ..Model::default()
        },
        ModelProfileKey::Generic,
    )
}

async fn resolve_validation_api_key(
    provider_id: &str,
    params: &devo_protocol::native::rpc_admin::ProviderValidateParams,
) -> anyhow::Result<Option<String>> {
    if let Some(api_key) = params.api_key.as_deref() {
        let trimmed = api_key.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }

    let Some(credential_id) = params.provider.credential.as_deref() else {
        return Ok(None);
    };
    let config_file = current_user_config_file().context("could not determine user config path")?;
    let config_dir = config_file
        .parent()
        .context("user config path has no parent directory")?;
    let auth = read_user_auth_config(&config_dir.join(AUTH_CONFIG_FILE_NAME))?;
    let credential = auth.credentials.get(credential_id).with_context(|| {
        format!(
            "provider {provider_id} references missing credential {credential_id} in user auth.json"
        )
    })?;
    match credential.kind {
        devo_core::AuthCredentialKind::ApiKey => Ok(Some(credential.value.clone())),
        devo_core::AuthCredentialKind::Oauth => crate::oauth_refresh::resolve_oauth_access(
            provider_id,
            credential_id,
            credential,
            config_dir,
        )
        .await
        .map(Some),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use devo_core::PresetModelCatalog;
    use devo_core::ProviderWireApi;
    use devo_protocol::ProviderInfo;
    use devo_protocol::ProviderModelInfo;
    use devo_protocol::native::rpc_admin::ProviderValidateParams;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn normalized_provider_id_trims_and_rejects_empty_names() {
        assert_eq!(
            normalized_provider_id(" openai "),
            Some("openai".to_string())
        );
        assert_eq!(normalized_provider_id("   "), None);
    }

    #[test]
    fn resolve_validation_model_preserves_runtime_catalog_profile() {
        let catalog = PresetModelCatalog::new(vec![Model {
            slug: "catalog-slug".to_string(),
            display_name: "Catalog Model".to_string(),
            context_window: 123_456,
            effective_context_window_percent: Some(70.0),
            max_tokens: Some(7_654),
            provider: ProviderWireApi::AnthropicMessages,
            ..Model::default()
        }]);

        let resolved = resolve_validation_model(
            &catalog,
            ProviderWireApi::OpenAIChatCompletions,
            "catalog-slug",
        );

        assert_eq!(
            resolved,
            (
                Model {
                    slug: "catalog-slug".to_string(),
                    display_name: "Catalog Model".to_string(),
                    context_window: 123_456,
                    effective_context_window_percent: Some(70.0),
                    max_tokens: Some(7_654),
                    provider: ProviderWireApi::OpenAIChatCompletions,
                    ..Model::default()
                },
                ModelProfileKey::CatalogSlug("catalog-slug".to_string()),
            )
        );
    }

    #[test]
    fn resolve_validation_model_uses_generic_profile_for_unknown_slug() {
        let resolved = resolve_validation_model(
            &PresetModelCatalog::default(),
            ProviderWireApi::OpenAIChatCompletions,
            "custom-catalog-slug",
        );

        assert_eq!(
            resolved,
            (
                Model {
                    slug: "custom-catalog-slug".to_string(),
                    display_name: "custom-catalog-slug".to_string(),
                    provider: ProviderWireApi::OpenAIChatCompletions,
                    ..Model::default()
                },
                ModelProfileKey::Generic,
            )
        );
    }

    /// Trace: L2-DES-APP-005, L2-DES-MODEL-001
    /// Verifies: provider validation applies provider custom header parsing before sending a validation request.
    #[tokio::test]
    async fn validate_provider_candidate_rejects_invalid_custom_headers() {
        let params = ProviderValidateParams {
            provider: ProviderInfo {
                id: "openai".to_string(),
                name: "openai".to_string(),
                description: None,
                base_url: Some("http://provider.example/v1".to_string()),
                credential: None,
                headers: BTreeMap::from([("bad header".to_string(), "value".to_string())]),
                options: None,
                request: None,
                compat: None,
                wire_apis: vec![ProviderWireApi::OpenAIChatCompletions],
                model_overrides: BTreeMap::new(),
                models: BTreeMap::from([(
                    "test-model".to_string(),
                    ProviderModelInfo {
                        wire_api: Some(ProviderWireApi::OpenAIChatCompletions),
                        ..ProviderModelInfo::default()
                    },
                )]),
                enabled: true,
            },
            model: "test-model".to_string(),
            api_key: None,
        };
        let catalog = PresetModelCatalog::new(Vec::new());

        let error = validate_provider_candidate(params, &catalog, ProviderHttpConfig::default())
            .await
            .expect_err("invalid headers should reject validation");

        assert_eq!(
            error.to_string(),
            "invalid provider custom header name `bad header`"
        );
    }
    #[test]
    fn bind_patches_custom_provider_in_place_without_seeding_stub() {
        let dir = tempfile::tempdir().expect("tempdir");
        let full = devo_core::ProviderConfigEntry {
            name: Some("CC Proxy".to_string()),
            base_url: Some("https://example/api".to_string()),
            credential: None,
            wire_api: Some(ProviderWireApi::AnthropicMessages),
            enabled: Some(true),
            models: BTreeMap::from([(
                "glm-5.3".to_string(),
                devo_core::ProviderModelConfig::default(),
            )]),
            ..devo_core::ProviderConfigEntry::default()
        };
        devo_core::write_provider_catalog_config(
            &dir.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME),
            &devo_core::ProviderConfigFile::default(),
        )
        .expect("write default provider catalog");
        devo_core::write_provider_catalog_config(
            &dir.path().join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME),
            &devo_core::ProviderConfigFile {
                providers: BTreeMap::from([("ccproxy".to_string(), full.clone())]),
                ..devo_core::ProviderConfigFile::default()
            },
        )
        .expect("write custom provider catalog");

        bind_provider_credential_id(dir.path(), "ccproxy", "ccproxy").expect("bind");

        let connection = devo_core::read_provider_catalog_config(
            &dir.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME),
        )
        .expect("read providers.json");
        let custom = devo_core::read_provider_catalog_config(
            &dir.path().join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME),
        )
        .expect("read custom-providers.json");
        // No sparse stub may be seeded into providers.json: the next start's
        // migrate_custom_providers_file would split it over the full entry.
        assert!(
            connection.providers.is_empty(),
            "providers.json must stay clean: {connection:?}"
        );
        let expected = devo_core::ProviderConfigEntry {
            credential: Some("ccproxy".to_string()),
            ..full
        };
        assert_eq!(
            custom.providers.get("ccproxy"),
            Some(&expected),
            "the custom entry keeps its adapter fields and gains the credential"
        );
    }

    #[test]
    fn bind_seeds_stub_for_unknown_provider_in_providers_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        devo_core::write_provider_catalog_config(
            &dir.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME),
            &devo_core::ProviderConfigFile::default(),
        )
        .expect("write default provider catalog");
        devo_core::write_provider_catalog_config(
            &dir.path().join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME),
            &devo_core::ProviderConfigFile::default(),
        )
        .expect("write custom provider catalog");

        bind_provider_credential_id(dir.path(), "fresh", "fresh").expect("bind");

        let connection = devo_core::read_provider_catalog_config(
            &dir.path().join(devo_core::PROVIDER_CONFIG_FILE_NAME),
        )
        .expect("read providers.json");
        assert_eq!(
            connection.providers.get("fresh"),
            Some(&devo_core::ProviderConfigEntry {
                name: Some("fresh".to_string()),
                credential: Some("fresh".to_string()),
                enabled: Some(true),
                ..devo_core::ProviderConfigEntry::default()
            })
        );
    }
}
