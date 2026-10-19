use devo_core::CatalogRefreshOutcome;
use devo_protocol::native::rpc_catalog::{
    CatalogRefreshPolicy, CatalogRefreshStatus, ModelCatalogRefreshParams,
    ModelCatalogRefreshResult,
};

use super::ServerRuntime;
use crate::{ProtocolErrorCode, SuccessResponse};

// Serialize catalog writes only; session actors and runtime maps stay independent.
static REFRESH_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl ServerRuntime {
    pub(crate) async fn refresh_remote_catalog(
        &self,
        config: &devo_core::CatalogConfig,
    ) -> CatalogRefreshOutcome {
        let home = self
            .deps
            .config_store
            .lock()
            .expect("config store mutex")
            .user_config_dir()
            .to_path_buf();
        let _guard = REFRESH_GATE.lock().await;
        let outcome = devo_core::refresh_remote_catalog(&home, config).await;
        if matches!(outcome, CatalogRefreshOutcome::Updated { .. }) {
            self.deps.invalidate_workspace_contexts();
        }
        outcome
    }

    pub(super) async fn handle_native_catalog_refresh(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<ModelCatalogRefreshParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid model/catalog/refresh params: {error}"),
                );
            }
        };
        let mut config = {
            let store = self.deps.config_store.lock().expect("config store mutex");
            store.effective_config().catalog.clone()
        };
        // Opening a picker may refresh stale data even when startup refresh is disabled.
        // Explicit refresh bypasses the TTL, but always respects offline/source settings.
        config.refresh_on_startup = true;
        if params.policy == CatalogRefreshPolicy::Force {
            config.refresh_interval_hours = 0;
        }
        let outcome = self.refresh_remote_catalog(&config).await;
        let result = match outcome {
            CatalogRefreshOutcome::Updated { .. } => ModelCatalogRefreshResult {
                status: CatalogRefreshStatus::Updated,
                message: None,
            },
            CatalogRefreshOutcome::CacheFresh | CatalogRefreshOutcome::SkippedStartupDisabled => {
                ModelCatalogRefreshResult {
                    status: CatalogRefreshStatus::Cached,
                    message: None,
                }
            }
            CatalogRefreshOutcome::SkippedOffline => ModelCatalogRefreshResult {
                status: CatalogRefreshStatus::Offline,
                message: None,
            },
            CatalogRefreshOutcome::Failed { message, .. } => ModelCatalogRefreshResult {
                status: CatalogRefreshStatus::Failed,
                message: Some(message),
            },
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result,
        })
        .expect("serialize model/catalog/refresh response")
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::TestRuntime;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn refresh_fetches_remote_then_uses_ttl_and_force_bypasses_it() {
        let home = tempfile::tempdir().expect("home");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind catalog source");
        let address = listener.local_addr().expect("source address");
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                "[catalog]\nsource = 'http://{address}/api.json'\nrefresh_on_startup = false\n"
            ),
        )
        .expect("write config");
        let source = tokio::spawn(async move {
            for version in 1..=2 {
                let (mut stream, _) = listener.accept().await.expect("accept refresh");
                let mut request = [0_u8; 4096];
                let mut received = Vec::new();
                loop {
                    let count = stream.read(&mut request).await.expect("read HTTP request");
                    if count == 0 {
                        break;
                    }
                    received.extend_from_slice(&request[..count]);
                    if received.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                assert!(received.starts_with(b"GET /api.json HTTP/1.1\r\n"));
                let body = json!({"anthropic": {"models": {"desktop-remote-qa": {
                    "name": format!("Remote model {version}"),
                    "tool_call": true,
                    "limit": {"context": 200000, "output": 8192}
                }}}})
                .to_string();
                let length = body.len();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("serve catalog");
            }
        });
        let runtime = TestRuntime::noop().runtime(home.path());
        let updated = json!({"status": "updated"});
        let (first, cached) = tokio::join!(
            runtime.handle_native_catalog_refresh(json!(1), json!({"policy": "ifStale"})),
            runtime.handle_native_catalog_refresh(json!(2), json!({"policy": "ifStale"})),
        );
        assert_eq!(first, json!({"id": 1, "result": updated}));
        assert_eq!(cached, json!({"id": 2, "result": {"status": "cached"}}));
        let forced = runtime
            .handle_native_catalog_refresh(json!(3), json!({"policy": "force"}))
            .await;
        assert_eq!(forced, json!({"id": 3, "result": updated}));
        source
            .await
            .expect("HTTP source completed exactly two requests");
        let listed = runtime.handle_native_model_list(json!(4), json!({})).await;
        let model = listed["result"]["models"]
            .as_array()
            .expect("models")
            .iter()
            .find(|model| model["slug"] == "anthropic/desktop-remote-qa")
            .expect("new remote model appears without restarting server");
        assert_eq!(
            json!({"slug": model["slug"], "name": model["displayName"], "context": model["contextWindow"]}),
            json!({"slug": "anthropic/desktop-remote-qa", "name": "Remote model 2", "context": 200000}),
        );
        // A broken update leaves the previous complete cache readable.
        let failed = runtime
            .handle_native_catalog_refresh(json!(5), json!({"policy": "force"}))
            .await;
        assert_eq!(failed["result"]["status"], "failed");
        let after_failure = runtime.handle_native_model_list(json!(6), json!({})).await;
        assert_eq!(listed["result"], after_failure["result"]);
    }

    #[tokio::test]
    async fn explicit_refresh_respects_offline_policy() {
        let home = tempfile::tempdir().expect("home");
        std::fs::write(
            home.path().join("config.toml"),
            "[catalog]\noffline = true\n",
        )
        .expect("write offline config");
        let runtime = TestRuntime::noop().runtime(home.path());
        assert_eq!(
            runtime
                .handle_native_catalog_refresh(json!(1), json!({"policy": "force"}))
                .await,
            json!({"id": 1, "result": {"status": "offline"}}),
        );
        assert_eq!(
            runtime
                .handle_native_provider_discover(json!(2), json!({"providerId": "openai-codex"}))
                .await,
            json!({"id": 2, "error": {"code": "PolicyDenied", "message": "provider discovery is disabled by catalog.offline", "data": {}}}),
        );
    }

    #[tokio::test]
    async fn startup_refresh_respects_disabled_policy() {
        let home = tempfile::tempdir().expect("home");
        let runtime = TestRuntime::noop().runtime(home.path());
        let config = devo_core::CatalogConfig {
            refresh_on_startup: false,
            ..Default::default()
        };
        assert_eq!(
            runtime.refresh_remote_catalog(&config).await,
            devo_core::CatalogRefreshOutcome::SkippedStartupDisabled
        );
        assert!(!home.path().join("cache/models.dev-api.json").exists());
    }

    #[tokio::test]
    async fn provider_refresh_keeps_manually_added_models() {
        let home = tempfile::tempdir().expect("home");
        let runtime = TestRuntime::noop().runtime(home.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider = devo_protocol::ProviderInfo {
            id: "qa-refresh".to_string(),
            name: "QA".to_string(),
            base_url: Some(format!("http://{address}/v1")),
            wire_apis: vec![devo_core::ProviderWireApi::OpenAIChatCompletions],
            models: [(
                "my-model".to_string(),
                devo_protocol::ProviderModelInfo {
                    origin: Some(devo_protocol::ProviderModelOrigin::User),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        runtime
            .deps
            .config_store
            .lock()
            .unwrap()
            .upsert_provider_connection_with_baseline(
                provider, /*default_model*/ None, /*small_model*/ None,
                /*api_key*/ None, /*builtin_baseline*/ None,
            )
            .unwrap();
        let source = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).await.unwrap();
            assert!(request[..count].starts_with(b"GET /v1/models HTTP/1.1\r\n"));
            let body = r#"{"data":[{"id":"fresh"}]}"#;
            let length = body.len();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}").as_bytes()).await.unwrap();
        });
        let response = runtime
            .handle_native_provider_discover(json!(1), json!({"providerId": "qa-refresh"}))
            .await;
        source.await.unwrap();
        let models = response["result"]["models"]
            .as_object()
            .expect("discovered models");
        assert_eq!(
            models
                .iter()
                .map(|(id, model)| (id.as_str(), model["origin"].as_str().unwrap()))
                .collect::<Vec<_>>(),
            vec![("fresh", "remote"), ("my-model", "user")]
        );
    }
}
