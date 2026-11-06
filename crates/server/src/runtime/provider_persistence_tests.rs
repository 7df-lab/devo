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

#[tokio::test]
async fn remote_models_remain_read_only_after_restart_and_settings_edits() -> Result<()> {
    let home = tempfile::tempdir()?;
    let runtime = TestRuntime::noop().runtime(home.path());
    let saved = runtime
        .handle_native_provider_upsert(
            json!(1),
            json!({
                "provider": {"id": "catalog-qa", "name": "Catalog QA", "enabled": true,
                    "baseUrl": "http://127.0.0.1:1/v1",
                    "wireApis": ["openai_chat_completions"], "models": {
                        "remote": {"name": "Remote", "origin": "remote"},
                        "manual": {"name": "Manual"}
                    }}, "defaultModel": "catalog-qa/remote"
            }),
        )
        .await;
    assert!(saved.get("error").is_none(), "save failed: {saved}");
    drop(runtime);
    let restarted = TestRuntime::noop().runtime(home.path());
    let before = restarted.handle_native_provider_list(json!(2)).await;
    assert_eq!(
        before["result"]["connectionModels"]["catalog-qa"],
        json!({
            "remote": {"origin": "remote", "wireApi": "openai_chat_completions"},
            "manual": {"name": "Manual", "origin": "user"}
        })
    );
    // Even an old or incorrect client cannot reclassify a discovered model.
    let edited = restarted
        .handle_native_provider_upsert(
            json!(3),
            json!({
                "provider": {"id": "catalog-qa", "name": "Catalog QA", "enabled": true,
                    "baseUrl": "http://127.0.0.1:1/v1",
                    "wireApis": ["openai_chat_completions"], "models": {
                        "remote": {"name": "Remote", "origin": "user"}
                    }}
            }),
        )
        .await;
    assert!(edited.get("error").is_none(), "edit failed: {edited}");
    let denied = restarted
        .handle_native_provider_model_remove(
            json!(4),
            json!({
                "providerId": "catalog-qa", "modelId": "remote"
            }),
        )
        .await;
    assert!(
        denied["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("read-only")),
        "removal was not denied: {denied}"
    );
    let after = restarted.handle_native_provider_list(json!(5)).await;
    assert_eq!(after["result"], before["result"]);
    let removed = restarted
        .handle_native_provider_model_remove(
            json!(6),
            json!({
                "providerId": "catalog-qa", "modelId": "manual"
            }),
        )
        .await;
    assert_eq!(
        removed,
        json!({"id": 6, "result": {"providerId": "catalog-qa", "modelId": "manual"}})
    );
    let remaining = restarted.handle_native_provider_list(json!(7)).await;
    assert_eq!(
        remaining["result"]["connectionModels"]["catalog-qa"],
        json!({
            "remote": {"origin": "remote", "wireApi": "openai_chat_completions"}
        })
    );
    Ok(())
}

#[tokio::test]
async fn explicit_remote_model_edits_persist_as_sparse_user_overrides() -> Result<()> {
    let home = tempfile::tempdir()?;
    let runtime = TestRuntime::noop().runtime(home.path());
    let saved = runtime
        .handle_native_provider_upsert(
            json!(1),
            json!({"provider": {
                "id": "edited-remote", "name": "Edited remote", "enabled": true,
                "baseUrl": "http://127.0.0.1:1/v1", "wireApis": ["openai_chat_completions"],
                "models": {
                    "selected": {"origin": "remote", "name": "Catalog name", "contextWindow": 128000,
                        "cost": {"input": 42}, "baseUrl": "http://127.0.0.1:2/v1", "requestModel": "wire-id"},
                    "unselected": {"origin": "remote", "name": "Not saved"}
                },
                "modelOverrides": {"selected": {"name": "My name", "contextWindow": 64000}}
            }}),
        )
        .await;
    assert!(saved.get("error").is_none(), "save failed: {saved}");
    drop(runtime);
    let persisted = devo_core::read_provider_catalog_config(
        &home
            .path()
            .join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME),
    )?;
    let persisted = serde_json::to_value(&persisted.providers["edited-remote"])?;
    assert_eq!(
        persisted["models"],
        json!({"selected": {
            "origin": "remote", "wire_api": "openai_chat_completions",
            "base_url": "http://127.0.0.1:2/v1", "request_model": "wire-id"
        }})
    );
    assert_eq!(
        persisted["model_overrides"],
        json!({"selected": {
            "name": "My name", "context_window": 64000
        }})
    );
    let restarted = TestRuntime::noop().runtime(home.path());
    let edit = restarted
        .handle_native_provider_upsert(
            json!(2),
            json!({"provider": {
                "id": "edited-remote", "name": "Edited remote", "enabled": true,
                "baseUrl": "http://127.0.0.1:1/v1", "wireApis": ["openai_chat_completions"],
                "modelOverrides": {"selected": {"contextWindow": 32000}}
            }}),
        )
        .await;
    assert!(edit.get("error").is_none(), "edit failed: {edit}");
    let persisted = devo_core::read_provider_catalog_config(
        &home
            .path()
            .join(devo_core::CUSTOM_PROVIDER_CONFIG_FILE_NAME),
    )?;
    let persisted = serde_json::to_value(&persisted.providers["edited-remote"])?;
    assert_eq!(
        persisted["model_overrides"],
        json!({"selected": {
            "name": "My name", "context_window": 32000
        }})
    );
    Ok(())
}
