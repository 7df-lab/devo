//! Python child model operations share the session's configured model resolver.

use devo_core::tools::AgentToolCoordinator;
use devo_protocol::{AgentToolPolicy, SpawnAgentParams};
use serde_json::{Value, json};

use super::kernel_host_bridge::HostBridge;

fn optional_string<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match params.get(key) {
        None => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() && value.trim() == value => {
            Ok(Some(value))
        }
        Some(_) => Err(format!(
            "{key} must be a nonempty string without surrounding whitespace"
        )),
    }
}

pub(super) async fn spawn(bridge: &HostBridge, params: &Value) -> Value {
    let result = async {
        let prompt = params
            .get("prompt")
            .and_then(Value::as_str)
            .filter(|prompt| !prompt.trim().is_empty())
            .ok_or_else(|| "rlm.run requires prompt".to_string())?;
        let kwargs = params
            .get("kwargs")
            .filter(|value| value.is_object())
            .ok_or_else(|| "rlm.run requires a kwargs object".to_string())?;
        let name =
            optional_string(kwargs, "name")?.ok_or_else(|| "rlm.run requires name".to_string())?;
        let model = optional_string(kwargs, "model")?.map(str::to_string);
        let thinking = optional_string(kwargs, "thinking")?.map(str::to_string);
        let runtime = bridge
            .runtime()
            .ok_or_else(|| "runtime not available for rlm.run".to_string())?;
        let spawned = runtime
            .spawn_agent(SpawnAgentParams {
                session_id: bridge.session_id,
                message: prompt.to_string(),
                model,
                thinking,
                fork_turns: Some("none".to_string()),
                max_turns: None,
                tool_policy: AgentToolPolicy::Inherit,
                ephemeral: false,
                nickname: Some(name.to_string()),
            })
            .await
            .map_err(|error| error.to_string())?;
        let session_dir = spawned
            .session_dir
            .filter(|dir| !dir.is_empty())
            .ok_or_else(|| "spawned child has no persisted session dir (ephemeral?)".to_string())?;
        Ok::<_, String>(json!({
            "rlm_child_id": spawned.child_session_id.to_string(),
            "name": spawned.agent_nickname,
            "session_dir": session_dir,
            "model": spawned.model.unwrap_or_default(),
        }))
    }
    .await;
    match result {
        Ok(result) => json!({"status": "ok", "result": result}),
        Err(error) => json!({"status": "error", "error": error}),
    }
}

pub(super) async fn find_models(bridge: &HostBridge, params: &Value) -> Value {
    let result = async {
        let query = match params.get("query") {
            None => "",
            Some(Value::String(query)) => query,
            Some(_) => return Err("query must be a string".to_string()),
        };
        let limit = match params.get("limit") {
            None => 8,
            Some(value) => value
                .as_u64()
                .filter(|limit| (1..=100).contains(limit))
                .ok_or_else(|| "limit must be an integer between 1 and 100".to_string())?,
        };
        let runtime = bridge
            .runtime()
            .ok_or_else(|| "runtime not available for rlm.find_models".to_string())?;
        let session = runtime
            .session(bridge.session_id)
            .await
            .ok_or_else(|| "session not available for rlm.find_models".to_string())?;
        let snapshot = session
            .turn_reservation_snapshot()
            .await
            .ok_or_else(|| "session not available for rlm.find_models".to_string())?;
        let models = snapshot
            .runtime_context
            .find_child_models(Some(query), limit as usize)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(json!({"models": models}))
    }
    .await;
    match result {
        Ok(result) => json!({"status": "ok", "result": result}),
        Err(error) => json!({"status": "error", "error": error}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn rejects_malformed_spawn_arguments_without_runtime() {
        let bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
        for value in [
            json!(null),
            json!(true),
            json!(8),
            json!(""),
            json!(" x/y "),
        ] {
            let reply = spawn(
                &bridge,
                &json!({"prompt":"task", "kwargs":{"name":"child", "model":value}}),
            )
            .await;
            assert_eq!(reply["status"], "error");
            assert!(reply["error"].as_str().unwrap().starts_with("model must"));
        }
        for value in [
            json!(null),
            json!(true),
            json!(0),
            json!(-1),
            json!(101),
            json!(1.5),
        ] {
            let reply = find_models(&bridge, &json!({"limit":value})).await;
            assert_eq!(
                reply,
                json!({"status":"error", "error":"limit must be an integer between 1 and 100"})
            );
        }
    }
}
