//! Independent root creation uses the canonical session constructor and Native turn admission.

use devo_core::SessionConfig;
use devo_protocol::native::session::SessionSettings;
use devo_protocol::{CollaborationMode, SessionStartParams};
use serde_json::{Value, json};

use super::kernel_host_bridge::HostBridge;

/// Initial safety/settings seed, installed before persistence and SessionStart hooks.
/// This is creation state, not an alternate settings-update protocol.
pub(crate) struct RootSessionSeed {
    pub config: SessionConfig,
    pub settings: SessionSettings,
    pub collaboration_mode: CollaborationMode,
}

pub(super) async fn create_session(bridge: &HostBridge, params: &Value) -> Value {
    let result = async {
        let prompt = params
            .get("prompt")
            .and_then(Value::as_str)
            .filter(|prompt| !prompt.trim().is_empty())
            .ok_or("create_session requires a nonempty prompt")?;
        let kwargs = params
            .get("kwargs")
            .and_then(Value::as_object)
            .ok_or("create_session requires kwargs")?;
        for key in kwargs.keys() {
            if !["name", "model", "thinking", "cwd"].contains(&key.as_str()) {
                return Err(format!("unsupported create_session option: {key}"));
            }
        }
        let option = |key: &str| -> Result<Option<&str>, String> {
            match kwargs.get(key) {
                None => Ok(None),
                Some(Value::String(value)) if !value.trim().is_empty() && value.trim() == value => {
                    Ok(Some(value))
                }
                _ => Err(format!(
                    "{key} must be a nonempty string without surrounding whitespace"
                )),
            }
        };
        let name = option("name")?;
        let selected_model = option("model")?;
        let thinking = option("thinking")?;
        let cwd = option("cwd")?;
        let runtime = bridge
            .runtime()
            .ok_or("runtime not available for rlm.create_session")?;
        if matches!(bridge.collaboration_mode, CollaborationMode::Plan) {
            return Err("create_session is blocked in Plan mode".to_string());
        }
        let caller = runtime
            .session(bridge.session_id)
            .await
            .ok_or("caller session is unavailable")?;
        let snapshot = caller
            .spawn_snapshot()
            .await
            .ok_or("caller snapshot is unavailable")?;
        if matches!(
            snapshot.parent_summary.collaboration_mode,
            CollaborationMode::Plan
        ) {
            return Err("create_session is blocked in Plan mode".to_string());
        }
        if snapshot.parent_summary.agent_path.is_some()
            || snapshot.parent_summary.ephemeral
            || matches!(
                snapshot.parent_summary.native.parent,
                Some(devo_protocol::native::session::SessionParent::Agent { .. })
            )
        {
            return Err("create_session requires a durable root session".to_string());
        }
        // Do not use independent sessions to escape the caller's filesystem
        // capability. A cross-workspace creation needs a separate approval flow.
        if let Some(cwd) = cwd {
            let requested = std::path::Path::new(cwd);
            let requested = if requested.is_absolute() {
                requested.to_path_buf()
            } else {
                bridge.cwd.join(requested)
            };
            if std::fs::canonicalize(requested).map_err(|error| error.to_string())?
                != std::fs::canonicalize(&bridge.cwd).map_err(|error| error.to_string())?
            {
                return Err(
                    "create_session currently requires the caller's working directory".to_string(),
                );
            }
        }
        // Like child spawn, inherit the current decision-point configuration,
        // including named variants and mid-turn model/thinking changes.
        let live_config = if let Some(stream) = runtime.active_stream_state(bridge.session_id).await
        {
            let stream = stream.lock().await;
            stream.turn_inline.as_ref().and_then(|inline| {
                inline
                    .live_turn_settings
                    .lock()
                    .ok()
                    .and_then(|live| live.turn_config.clone())
            })
        } else {
            None
        };
        let parent_model = live_config
            .as_ref()
            .map(|config| {
                let base = config
                    .model_binding_id
                    .as_ref()
                    .unwrap_or(&config.model.slug);
                config
                    .variant
                    .as_ref()
                    .map(|variant| format!("{base}/{variant}"))
                    .unwrap_or_else(|| base.clone())
            })
            .or_else(|| {
                super::session_model_selection(&snapshot.parent_summary).map(str::to_string)
            })
            .ok_or("caller has no model binding")?;
        let parent_thinking = match live_config.as_ref() {
            Some(config) => config.reasoning_effort_selection.as_deref(),
            None => snapshot.parent_summary.settings.reasoning_effort.as_deref(),
        };
        let (_, resolved) = snapshot
            .runtime_context
            .resolve_child_model(selected_model, &parent_model, thinking, parent_thinking)
            .map_err(|error| error.to_string())?;
        let model = resolved
            .model_binding_id
            .clone()
            .unwrap_or_else(|| resolved.model.slug.clone());
        let model_selection = resolved
            .variant
            .as_ref()
            .map(|variant| format!("{model}/{variant}"))
            .unwrap_or_else(|| model.clone());
        let mut settings = snapshot.parent_summary.native.settings.clone();
        settings.reasoning_effort = resolved.reasoning_effort_selection;
        // A root is independently durable and visible; it has no parent usage
        // ledger, ancestry, shared journal, or implicit parent completion notice.
        let connection = runtime
            .active_turns
            .active_connection_id(bridge.session_id)
            .await;
        let response = runtime
            .start_session_with_seed(
                connection.unwrap_or(0),
                json!("rlm.create_session"),
                SessionStartParams {
                    cwd: bridge.cwd.clone(),
                    additional_directories: snapshot.parent_summary.additional_directories.clone(),
                    ephemeral: false,
                    title: name.map(str::to_string),
                    model: Some(model_selection),
                    model_binding_id: None,
                },
                snapshot.parent_tool_registry,
                Some(RootSessionSeed {
                    config: snapshot.parent_config,
                    settings,
                    collaboration_mode: bridge.collaboration_mode,
                }),
            )
            .await;
        if let Some(error) = response.get("error") {
            return Err(format!("create_session failed: {error}"));
        }
        let session: devo_protocol::native::session::Session =
            serde_json::from_value(response["result"]["session"].clone())
                .map_err(|error| format!("invalid created session: {error}"))?;
        let session_id = session.id;
        let handle = runtime
            .session(session_id)
            .await
            .ok_or_else(|| format!("created session {session_id} is unavailable"))?;
        let session_file = handle
            .rollout_path()
            .await
            .flatten()
            .ok_or_else(|| format!("created session {session_id} has no journal"))?;
        let turn = runtime
            .handle_turn_start_for_connection(
                connection,
                json!("rlm.create_session.turn"),
                json!({
                    "sessionId": session_id, "input": [{"type":"text", "text":prompt}],
                    "idempotencyKey": format!("rlm-create-{session_id}"),
                }),
            )
            .await;
        if let Some(error) = turn.get("error") {
            // Creation is durable. Do not hide the id or retry with another root.
            return Err(format!(
                "created session {session_id}, but its initial turn was not admitted: {error}"
            ));
        }
        Ok::<_, String>(json!({
            "active_session_id": session_id.to_string(), "session_id": session_id.to_string(),
            "name": name.map(str::to_string).unwrap_or_else(|| session_id.to_string()),
            "session_file": session_file, "model": model,
        }))
    }
    .await;
    match result {
        Ok(result) => json!({"status":"ok", "result":result}),
        Err(error) => json!({"status":"error", "error":error}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;

    #[tokio::test]
    async fn root_inherits_restrictive_safety_and_invalid_requests_create_nothing() {
        let home = tempfile::TempDir::new().unwrap();
        let runtime = crate::test_support::TestRuntime::noop().runtime(home.path());
        let started = runtime
            .start_session_with_registry(
                0,
                json!(1),
                SessionStartParams {
                    cwd: home.path().to_path_buf(),
                    additional_directories: Vec::new(),
                    ephemeral: false,
                    title: Some("caller".into()),
                    model: None,
                    model_binding_id: None,
                },
                None,
            )
            .await;
        let caller_id: devo_protocol::SessionId =
            serde_json::from_value(started["result"]["session"]["id"].clone()).unwrap();
        let updated = runtime
            .handle_native_session_metadata_update(
                json!(2),
                json!({
                    "sessionId": caller_id, "expectedVersion":0,
                    "settings":{"permissionProfile":"default", "sandboxProfile":"strict"}
                }),
            )
            .await;
        assert!(updated.get("error").is_none(), "{updated}");
        let mut bridge = HostBridge::for_tests(caller_id);
        bridge.cwd = home.path().to_path_buf();
        bridge.runtime = Arc::downgrade(&runtime);
        let parent = runtime
            .session(caller_id)
            .await
            .unwrap()
            .spawn_snapshot()
            .await
            .unwrap();
        assert_eq!(
            parent.parent_summary.native.settings.permission_profile,
            devo_protocol::native::model::PermissionProfile::Default
        );
        for params in [
            json!({"prompt":"task","kwargs":{"cwd":".."}}),
            json!({"prompt":"task","kwargs":{"model":"unavailable/model"}}),
            json!({"prompt":"task","kwargs":{"unexpected":true}}),
        ] {
            assert_eq!(
                create_session(&bridge, &params).await["status"],
                json!("error")
            );
            assert_eq!(runtime.deps.db.list_root_sessions().unwrap().len(), 1);
        }
        bridge.collaboration_mode = CollaborationMode::Plan;
        assert_eq!(
            create_session(&bridge, &json!({"prompt":"task","kwargs":{}})).await,
            json!({"status":"error","error":"create_session is blocked in Plan mode"})
        );
        assert_eq!(runtime.deps.db.list_root_sessions().unwrap().len(), 1);
        bridge.collaboration_mode = CollaborationMode::Build;
        let created = create_session(
            &bridge,
            &json!({"prompt":"task","kwargs":{"name":"independent"}}),
        )
        .await;
        assert_eq!(created["status"], json!("ok"), "{created}");
        let id = serde_json::from_value(created["result"]["session_id"].clone()).unwrap();
        let child = runtime
            .session(id)
            .await
            .unwrap()
            .spawn_snapshot()
            .await
            .unwrap();
        assert_eq!(
            child.parent_config.permission_mode,
            parent.parent_config.permission_mode
        );
        assert_eq!(
            child.parent_config.permission_profile,
            parent.parent_config.permission_profile
        );
        assert_eq!(
            child.parent_config.sandbox_profile,
            parent.parent_config.sandbox_profile
        );
        assert_eq!(child.parent_summary.native.parent, None);
        assert_eq!(runtime.deps.db.list_root_sessions().unwrap().len(), 2);
        runtime.shutdown().await;
        drop(runtime);
        let restored = crate::test_support::TestRuntime::noop().runtime(home.path());
        restored.load_persisted_sessions().await.unwrap();
        let resumed = restored.get_or_load_parent_session(id).await.unwrap();
        let resumed = resumed.spawn_snapshot().await.unwrap();
        assert_eq!(
            resumed.parent_config.permission_profile,
            parent.parent_config.permission_profile
        );
        assert_eq!(
            resumed.parent_config.sandbox_profile.as_deref(),
            Some("strict")
        );
    }
}
