//! One-shot CLI adapter to the same host-owned harness operations as the server.

use std::path::Path;

use devo_core::tools::{SandboxPermissionRequest, ToolPermissionRequest, ToolRuntime};
use devo_harness::{
    HarnessCommand, HarnessScope, HarnessState, canonical_harness_path, mutate_harness,
};
use devo_protocol::CollaborationMode;
use devo_safety::ResourceKind;
use serde_json::{Value, json};

use super::error_reply;

pub(super) async fn handle_harness(
    runtime: &ToolRuntime,
    request_id: String,
    action: &str,
    params: &Value,
    home: &Path,
) -> Value {
    if !params.is_object() {
        return error_reply("harness request params must be an object");
    }
    let operation = action.strip_prefix("harness.").unwrap_or("");
    let mutation = match operation {
        "load" => false,
        "create" | "update" | "upsert" | "delete" | "record_refinement" => true,
        _ => return error_reply(format!("unknown harness action: {action}")),
    };
    let context = runtime.context();
    if mutation && context.collaboration_mode == CollaborationMode::Plan {
        return error_reply(format!("denied: {action} blocked in Plan mode"));
    }
    let scope: HarnessScope = match params.get("scope") {
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(scope) => scope,
            Err(error) => return error_reply(format!("invalid harness scope: {error}")),
        },
        None => HarnessScope::Local,
    };
    let root = match scope {
        HarnessScope::Local => match context.session_dir.as_deref() {
            Some(root) => root,
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
        HarnessScope::Global => home,
    };
    let path = match canonical_harness_path(&HarnessState::file_path(root)) {
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
    let request = ToolPermissionRequest {
        tool_call_id: request_id,
        tool_name: if mutation { "write" } else { "read" }.into(),
        input: params.clone(),
        cwd: context.cwd.clone(),
        session_id: context.session_id,
        turn_id: context.turn_id,
        resource: if mutation {
            ResourceKind::FileWrite
        } else {
            ResourceKind::FileRead
        },
        action_summary: format!("{action} {}", path.display()),
        justification: None,
        path: Some(path.clone()),
        host: None,
        target: None,
        command_prefix: None,
        command_argv: None,
        command_pattern: None,
        sandbox_permissions: SandboxPermissionRequest::Default,
    };
    if let Err(error) = runtime.permission_checker().check(request).await {
        return error_reply(error);
    }
    if runtime.cancel_token().is_cancelled() {
        return error_reply("cancelled");
    }
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
    use devo_core::tools::{PermissionChecker, ToolRegistry, ToolRuntimeContext};
    use pretty_assertions::assert_eq;
    use std::sync::Arc;

    #[tokio::test]
    async fn cli_local_harness_is_explicitly_ephemeral() {
        let runtime = ToolRuntime::new_without_permissions(Arc::new(ToolRegistry::new()));
        assert_eq!(
            super::super::dispatch_host_request(
                &runtime,
                "load".into(),
                json!({"type":"harness.load"})
            )
            .await,
            json!({"status":"ok", "result":{"state":HarnessState::default(), "file_path":null, "scope":"local"}})
        );
        assert_eq!(super::super::dispatch_host_request(&runtime, "create".into(), json!({
            "type":"harness.create", "kind":"memory", "id":"m", "title":"M", "content":"value"
        })).await, json!({"status":"error", "error":"This session has no persistent local harness store; pass global_=True to persist across sessions."}));
    }

    #[tokio::test]
    async fn cli_global_harness_uses_shared_store_and_runtime_policy() {
        let home = tempfile::tempdir().unwrap();
        let runtime = ToolRuntime::new(
            Arc::new(ToolRegistry::new()),
            super::super::permission_checker(),
        );
        let reply = handle_harness(
            &runtime,
            "create".into(),
            "harness.create",
            &json!({
                "scope":"global", "kind":"memory", "id":"m", "title":"M", "content":"value"
            }),
            home.path(),
        )
        .await;
        assert_eq!(reply["status"], "ok");
        let state = HarnessState::load(&HarnessState::file_path(home.path())).unwrap();
        assert_eq!(reply["result"]["state"], json!(state));
        assert_eq!(
            state.entries["memory"]["m"].scope,
            Some(HarnessScope::Global)
        );

        let denied = ToolRuntime::new(
            Arc::new(ToolRegistry::new()),
            PermissionChecker::new(|_| Box::pin(async { Err("test denial".into()) })),
        );
        assert_eq!(
            handle_harness(
                &denied,
                "delete".into(),
                "harness.delete",
                &json!({
                    "scope":"global", "kind":"memory", "id":"m"
                }),
                home.path()
            )
            .await,
            json!({"status":"error", "error":"test denial"})
        );
        assert_eq!(
            HarnessState::load(&HarnessState::file_path(home.path())).unwrap(),
            state
        );
    }

    #[tokio::test]
    async fn cli_plan_denies_global_mutation_but_allows_reads() {
        let home = tempfile::tempdir().unwrap();
        let runtime = ToolRuntime::new_with_context(
            Arc::new(ToolRegistry::new()),
            super::super::permission_checker(),
            ToolRuntimeContext {
                collaboration_mode: CollaborationMode::Plan,
                ..ToolRuntimeContext::default()
            },
        );
        assert_eq!(
            handle_harness(
                &runtime,
                "delete".into(),
                "harness.delete",
                &json!({
                    "scope":"global", "kind":"memory", "id":"m"
                }),
                home.path()
            )
            .await,
            json!({"status":"error", "error":"denied: harness.delete blocked in Plan mode"})
        );
        let reply = handle_harness(
            &runtime,
            "load".into(),
            "harness.load",
            &json!({"scope":"global"}),
            home.path(),
        )
        .await;
        assert_eq!(reply["result"]["state"], json!(HarnessState::default()));
        assert!(!HarnessState::file_path(home.path()).exists());
    }
}
