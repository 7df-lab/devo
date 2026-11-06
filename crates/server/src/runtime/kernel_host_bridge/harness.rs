//! Harness persistence is host-owned. Python supplies operations, never paths.

use devo_harness::{
    HarnessCommand, HarnessScope, HarnessState, canonical_harness_path, mutate_harness,
};
use serde_json::{Value, json};

use super::{
    HostBridge, check_or_error, deny_plan_mutation, error_reply, permission_request,
    plan_mode_denies_mutation, synthetic_tool_call_id,
};
use devo_safety::ResourceKind;

pub(crate) async fn handle_harness(bridge: &HostBridge, action: &str, params: &Value) -> Value {
    if !params.is_object() {
        return error_reply("harness request params must be an object");
    }
    let operation = action.strip_prefix("harness.").unwrap_or("");
    let mutation = match operation {
        "load" => false,
        "create" | "update" | "upsert" | "delete" | "record_refinement" => true,
        _ => return error_reply(format!("unknown harness action: {action}")),
    };
    if mutation && plan_mode_denies_mutation(bridge) {
        return deny_plan_mutation(action);
    }
    let scope: HarnessScope = match params.get("scope") {
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(scope) => scope,
            Err(error) => return error_reply(format!("invalid harness scope: {error}")),
        },
        None => HarnessScope::Local,
    };
    // Only host-bound roots are used. No Python state_dir/path override can
    // select another session's store or a caller-controlled arbitrary file.
    let root = match scope {
        HarnessScope::Local => match &bridge.session_dir {
            Some(root) => root.clone(),
            None if !mutation => {
                return json!({"status":"ok", "result": {
                    "state": HarnessState::default(), "file_path": null, "scope": scope
                }});
            }
            None => {
                return error_reply(
                    "This session has no persistent local harness store; pass global_=True to persist across sessions.",
                );
            }
        },
        HarnessScope::Global => match devo_util_paths::find_devo_home() {
            Ok(root) => root,
            Err(error) => return error_reply(error.to_string()),
        },
    };
    let path = match canonical_harness_path(&HarnessState::file_path(&root)) {
        Ok(path) => path,
        Err(error) => return error_reply(error.to_string()),
    };
    let command = if mutation {
        let mut input = params.clone();
        input["op"] = json!(operation);
        match serde_json::from_value::<HarnessCommand>(input) {
            Ok(command) => Some(command),
            Err(error) => return error_reply(format!("invalid {action} request: {error}")),
        }
    } else {
        None
    };
    let request = permission_request(
        bridge,
        synthetic_tool_call_id(action),
        if mutation { "write" } else { "read" },
        params.clone(),
        if mutation {
            ResourceKind::FileWrite
        } else {
            ResourceKind::FileRead
        },
        format!("{action} {}", path.display()),
        Some(path.clone()),
        /*target*/ None,
    );
    if let Err(reply) = check_or_error(bridge, request).await {
        return reply;
    }
    if bridge.cancel_token.is_cancelled() {
        return error_reply("cancelled");
    }
    // File locking and persistence must not block the actor or Tokio executor.
    match tokio::task::spawn_blocking(move || {
        let mut result = match command {
            Some(command) => {
                mutate_harness(&path, scope, command).map_err(|error| error.to_string())?
            }
            None => json!({"state": HarnessState::load(&path).map_err(|error| error.to_string())?}),
        };
        result["file_path"] = json!(path);
        result["scope"] = json!(scope);
        Ok::<_, String>(result)
    })
    .await
    {
        Ok(Ok(result)) => json!({"status":"ok", "result":result}),
        Ok(Err(error)) => error_reply(error),
        Err(error) => error_reply(format!("harness worker failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_core::tools::PermissionChecker;
    use devo_protocol::CollaborationMode;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn plan_and_ephemeral_mutations_fail_without_creating_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
        let params = json!({"kind":"memory","id":"m","title":"M","content":"value"});
        assert_eq!(
            handle_harness(&bridge, "harness.create", &params).await,
            json!({
                "status":"error", "error":"This session has no persistent local harness store; pass global_=True to persist across sessions."
            })
        );
        assert_eq!(
            handle_harness(&bridge, "harness.load", &json!({})).await,
            json!({
                "status":"ok", "result":{"state":HarnessState::default(), "file_path":null, "scope":"local"}
            })
        );
        bridge.session_dir = Some(dir.path().to_path_buf());
        bridge.collaboration_mode = CollaborationMode::Plan;
        assert_eq!(
            handle_harness(&bridge, "harness.create", &params).await,
            json!({
                "status":"error", "error":"denied: harness.create blocked in Plan mode"
            })
        );
        assert!(!HarnessState::file_path(dir.path()).exists());
    }

    #[tokio::test]
    async fn host_crud_uses_bound_session_and_checks_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let mut bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
        bridge.session_dir = Some(dir.path().to_path_buf());
        let params =
            json!({"kind":"memory","id":"m","title":"M","content":"value", "file_path":"ignored"});
        let reply = handle_harness(&bridge, "harness.create", &params).await;
        assert_eq!(reply["status"], "ok");
        let before = HarnessState::load(&HarnessState::file_path(dir.path())).unwrap();
        bridge.permission =
            PermissionChecker::new(|_| Box::pin(async { Err("test denial".into()) }));
        let reply = handle_harness(
            &bridge,
            "harness.delete",
            &json!({"kind":"memory", "id":"m"}),
        )
        .await;
        assert_eq!(reply["status"], "error");
        assert_eq!(
            HarnessState::load(&HarnessState::file_path(dir.path())).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn malformed_payload_is_rejected_before_dispatch() {
        let bridge = HostBridge::for_tests("ses_00000000-0000-0000-0000-000000000001");
        assert_eq!(
            handle_harness(&bridge, "harness.create", &json!([])).await,
            json!({"status":"error", "error":"harness request params must be an object"})
        );
    }
}
