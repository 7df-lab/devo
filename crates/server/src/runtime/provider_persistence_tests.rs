//! Desktop connections must remain visible after a fresh Native runtime loads
//! the same home directory. No network calls or process environment mutations.
use std::sync::Arc;

use anyhow::Result;
use devo_core::PresetModelCatalog;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::test_support::TestRuntime;

#[tokio::test]
async fn oauth_and_custom_connections_survive_native_runtime_restart() -> Result<()> {
    let home = tempfile::tempdir()?;
    let runtime = TestRuntime::noop()
        .catalog(Arc::new(PresetModelCatalog::load()?))
        .runtime(home.path());
    let oauth = runtime
        .handle_native_credential_set(
            json!(1),
            json!({
                "provider": "openai-codex", "kind": "oauth", "access": "local-qa-token",
                "expiresAt": chrono::Utc::now().timestamp() + 86400,
            }),
        )
        .await;
    assert!(oauth.get("error").is_none(), "OAuth save failed: {oauth}");
    let custom = runtime
        .handle_native_provider_upsert(
            json!(2),
            json!({
                "provider": {"id": "settings-qa", "name": "Settings QA", "enabled": true,
                    "baseUrl": "http://127.0.0.1:1/v1", "wireApis": ["openai_chat_completions"],
                    "models": {"qa-model": {"name": "QA Model"}}}, "apiKey": "local-qa-key",
            }),
        )
        .await;
    assert!(
        custom.get("error").is_none(),
        "custom save failed: {custom}"
    );
    let before = runtime.handle_native_provider_list(json!(3)).await;
    let credentials = runtime.handle_native_credential_list(json!(4)).await;
    assert_eq!(
        before["result"]["connectedProviderIds"],
        json!(["openai-codex", "settings-qa"])
    );
    drop(runtime);
    let restarted = TestRuntime::noop()
        .catalog(Arc::new(PresetModelCatalog::load()?))
        .runtime(home.path());
    let after = restarted.handle_native_provider_list(json!(5)).await;
    let after_credentials = restarted.handle_native_credential_list(json!(6)).await;
    assert_eq!(after["result"], before["result"]);
    assert_eq!(after_credentials["result"], credentials["result"]);
    let models = restarted
        .handle_native_model_list(json!(7), json!({}))
        .await;
    assert!(
        models["result"]["models"]
            .as_array()
            .expect("models")
            .iter()
            .any(|model| model["slug"] == "settings-qa/qa-model")
    );
    Ok(())
}
