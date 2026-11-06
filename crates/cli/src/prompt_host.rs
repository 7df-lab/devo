use std::sync::Arc;

use devo_core::tools::{PermissionChecker, PermissionGrant, ToolCall, ToolContent, ToolRuntime};
use devo_kernel::HostRequestHandler;
use serde_json::{Map, Value, json};

/// Preserve the headless command's static-allow policy while delivering requested grants.
pub(crate) fn permission_checker() -> PermissionChecker {
    PermissionChecker::new(|request| {
        Box::pin(async move {
            Ok(PermissionGrant {
                bypass_sandbox: request.sandbox_permissions.bypasses_sandbox(),
                already_approved: false,
                sandbox_permission_overlay: request.sandbox_permissions.overlay(),
            })
        })
    })
}

pub(crate) fn host_request_handler(runtime: Arc<ToolRuntime>) -> HostRequestHandler {
    Arc::new(move |request_id, data| {
        let runtime = Arc::clone(&runtime);
        Box::pin(async move { dispatch_host_request(&runtime, request_id, data).await })
    })
}

/// Keep detached handle polling available between Python cells, without allowing new work.
pub(crate) fn idle_host_request_handler(session_id: String) -> HostRequestHandler {
    Arc::new(move |_request_id, data| {
        let session_id = session_id.clone();
        Box::pin(async move {
            let (action, params) = devo_kernel::parse_host_request_data(&data);
            match action.as_str() {
                "bash.poll" | "bash.kill" | "bash.forget" => {
                    devo_core::tools::host_bash::control(&session_id, &action, &params)
                }
                // One-shot prompts have no persisted completion-notice inbox.
                "bash.completed" | "bash.consumed" => {
                    json!({"status":"ok", "result":{"accepted":true}})
                }
                _ => error_reply(format!("idle host request denied: {action}")),
            }
        })
    })
}

async fn dispatch_host_request(runtime: &ToolRuntime, request_id: String, data: Value) -> Value {
    let Some(action) = data
        .get("type")
        .or_else(|| data.get("action"))
        .and_then(Value::as_str)
    else {
        return error_reply("host request is missing a type");
    };

    if matches!(action, "bash.poll" | "bash.kill" | "bash.forget") {
        return runtime.control_host_bash(action, &data);
    }
    if action == "bash.start" {
        let mut input = data.as_object().cloned().unwrap_or_default();
        input.remove("type");
        input.remove("action");
        let result = runtime
            .start_host_bash(request_id, Value::Object(input))
            .await;
        return if result.is_error {
            error_reply(result.content.into_string())
        } else {
            json!({ "status": "ok", "result": content_as_json(result.content) })
        };
    }

    let (tool_name, input) = match action {
        "bash" => {
            let mut input = data.as_object().cloned().unwrap_or_default();
            input.remove("type");
            input.remove("action");
            ("bash", Value::Object(input))
        }
        "web.search" => (
            "web_search",
            json!({ "query": data.get("query").cloned().unwrap_or(Value::Null) }),
        ),
        "web.fetch" => {
            let mut input = Map::new();
            for key in ["url", "format", "timeout"] {
                if let Some(value) = data.get(key) {
                    input.insert(key.to_string(), value.clone());
                }
            }
            ("webfetch", Value::Object(input))
        }
        "plan.update" => {
            return error_reply(
                "plan display is unavailable in the one-shot `devo prompt` command",
            );
        }
        "question" => {
            return error_reply("interactive user questions are unavailable in `devo prompt`");
        }
        _ => return error_reply(format!("unsupported host request: {action}")),
    };

    let results = runtime
        .execute_batch(&[ToolCall {
            id: request_id,
            name: tool_name.to_string(),
            input,
        }])
        .await;
    let Some(result) = results.into_iter().next() else {
        return error_reply("host tool returned no result");
    };
    if result.is_error {
        return error_reply(result.content.into_string());
    }

    json!({ "status": "ok", "result": content_as_json(result.content) })
}

fn content_as_json(content: ToolContent) -> Value {
    match content {
        ToolContent::Text(text) => json!({ "content": text }),
        ToolContent::Json(value) => value,
        ToolContent::Mixed { text, json } => {
            let mut result = json
                .as_ref()
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            if let Some(text) = text {
                result.insert("content".to_string(), Value::String(text));
            }
            if let Some(metadata) = json.filter(|value| !value.is_object()) {
                result.insert("metadata".to_string(), metadata);
            }
            Value::Object(result)
        }
    }
}

fn error_reply(error: impl Into<String>) -> Value {
    json!({ "status": "error", "error": error.into() })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use devo_core::tools::{
        ToolCallError, ToolContext, ToolHandler, ToolProgressSender, ToolRegistry,
        ToolRegistryBuilder, ToolResult, ToolResultContent, ToolRuntime, ToolSpec,
    };
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::{content_as_json, dispatch_host_request, permission_checker};

    #[tokio::test]
    async fn headless_policy_delivers_requested_grants_as_static_allows() {
        use devo_core::tools::{
            AdditionalSandboxPermissions, NetworkPermission, PermissionGrant,
            SandboxPermissionRequest, ToolPermissionRequest,
        };
        let overlay = AdditionalSandboxPermissions {
            network: NetworkPermission::Enabled,
            read_paths: Vec::new(),
            write_paths: vec![std::env::temp_dir()],
        };
        for (permissions, expected) in [
            (
                SandboxPermissionRequest::Default,
                PermissionGrant::default(),
            ),
            (
                SandboxPermissionRequest::FullEscalation,
                PermissionGrant {
                    bypass_sandbox: true,
                    already_approved: false,
                    sandbox_permission_overlay: None,
                },
            ),
            (
                SandboxPermissionRequest::AdditionalPermissions(overlay.clone()),
                PermissionGrant {
                    bypass_sandbox: false,
                    already_approved: false,
                    sandbox_permission_overlay: Some(overlay),
                },
            ),
        ] {
            let request = ToolPermissionRequest {
                tool_call_id: "headless-shell".to_string(),
                tool_name: "bash".to_string(),
                input: json!({"command":"echo probe"}),
                cwd: std::env::temp_dir(),
                session_id: "ses_headless_shell".into(),
                turn_id: None,
                resource: devo_safety::ResourceKind::ShellExec,
                action_summary: "headless shell probe".to_string(),
                justification: None,
                path: None,
                host: None,
                target: Some("echo probe".to_string()),
                command_prefix: None,
                command_argv: None,
                command_pattern: None,
                sandbox_permissions: permissions,
            };
            assert_eq!(permission_checker().check(request).await, Ok(expected));
        }
    }

    struct ShellInputProbe {
        spec: ToolSpec,
    }

    #[async_trait::async_trait]
    impl ToolHandler for ShellInputProbe {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }

        async fn handle(
            &self,
            _ctx: ToolContext,
            input: serde_json::Value,
            _progress: Option<ToolProgressSender>,
        ) -> Result<ToolResult, ToolCallError> {
            Ok(ToolResult::success(ToolResultContent::Json(input), "probe"))
        }
    }

    #[tokio::test]
    async fn prompt_host_forwards_shell_requests_to_the_hidden_runtime_handler() {
        let mut builder = ToolRegistryBuilder::new();
        let mut spec = devo_core::tools::handlers::ShellCommandHandler::new()
            .spec()
            .clone();
        spec.name = "bash".to_string();
        builder.push_spec(spec.clone());
        builder.register_handler("bash", Arc::new(ShellInputProbe { spec }));
        builder.restrict_model_exposure_to(&["ipython"]);
        let runtime = ToolRuntime::new_without_permissions(Arc::new(builder.build()));
        for action in ["bash"] {
            assert_eq!(
                dispatch_host_request(
                    &runtime,
                    "shell-probe".to_string(),
                    json!({
                        "type": action,
                        "command": "echo headless",
                        "sandbox_permissions": "require_escalated",
                        "justification": "May I run the headless probe?",
                        "timeout_ms": 1000,
                        "prefix_rule": ["echo"],
                    })
                )
                .await,
                json!({
                    "status": "ok",
                    "result": {
                        "command": "echo headless",
                        "sandbox_permissions": "require_escalated",
                        "justification": "May I run the headless probe?",
                        "timeout_ms": 1000,
                        "prefix_rule": ["echo"],
                    }
                })
            );
        }
    }

    #[tokio::test]
    async fn prompt_host_start_preserves_runtime_permission_denial() {
        let mut builder = ToolRegistryBuilder::new();
        let handler = devo_core::tools::handlers::ShellCommandHandler::new();
        let mut spec = handler.spec().clone();
        spec.name = "bash".to_string();
        builder.push_spec(spec);
        builder.register_handler("bash", Arc::new(handler));
        let runtime = ToolRuntime::new(
            Arc::new(builder.build()),
            devo_core::tools::PermissionChecker::new(|request| {
                Box::pin(async move {
                    assert_eq!(request.input["command"], json!("echo denied"));
                    Err("declined".to_string())
                })
            }),
        );
        assert_eq!(
            dispatch_host_request(
                &runtime,
                "denied".to_string(),
                json!({"type":"bash.start", "command":"echo denied"})
            )
            .await,
            json!({"status":"error", "error":"permission denied: declined"}),
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prompt_host_shell_handles_complete_fail_and_cancel() {
        let mut builder = ToolRegistryBuilder::new();
        let handler = devo_core::tools::handlers::ShellCommandHandler::new();
        let mut spec = handler.spec().clone();
        spec.name = "bash".to_string();
        builder.push_spec(spec);
        builder.register_handler("bash", Arc::new(handler));
        let runtime = ToolRuntime::new_with_context(
            Arc::new(builder.build()),
            permission_checker(),
            devo_core::tools::ToolRuntimeContext {
                session_id: "ses_cli_lifecycle".into(),
                cwd: std::env::current_dir().expect("test cwd"),
                ..Default::default()
            },
        );
        for (command, expected_exit) in [
            ("printf cli-ok", Some(0)),
            ("exit 7", Some(7)),
            ("sleep 30", None),
        ] {
            let start = dispatch_host_request(
                &runtime,
                "start".to_string(),
                json!({"type":"bash.start", "command":command}),
            )
            .await;
            assert_eq!(start["status"], json!("ok"), "{start}");
            let handle_id = start["result"]["handle_id"].clone();
            if expected_exit.is_none() {
                let killed = dispatch_host_request(
                    &runtime,
                    "kill".to_string(),
                    json!({"type":"bash.kill", "handle_id":handle_id}),
                )
                .await;
                assert_eq!(killed["status"], json!("ok"));
            }
            let final_reply = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let reply = dispatch_host_request(
                        &runtime,
                        "poll".to_string(),
                        json!({"type":"bash.poll", "handle_id":handle_id}),
                    )
                    .await;
                    if reply["result"]["running"] == false {
                        break reply;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("shell completion");
            if let Some(exit) = expected_exit {
                assert_eq!(
                    final_reply["result"]["exit_code"],
                    json!(exit),
                    "{final_reply}"
                );
            } else {
                assert!(final_reply["result"]["error"].is_string(), "{final_reply}");
            }
            assert_eq!(
                dispatch_host_request(
                    &runtime,
                    "forget".to_string(),
                    json!({"type":"bash.forget", "handle_id":handle_id})
                )
                .await,
                json!({"status":"ok", "result":{"forgotten":true}})
            );
        }
        devo_core::tools::host_bash::cancel_session("ses_cli_lifecycle");
    }

    #[test]
    fn mixed_tool_content_preserves_text_and_metadata_for_python() {
        assert_eq!(
            content_as_json(devo_core::tools::ToolContent::Mixed {
                text: Some("page text".to_string()),
                json: Some(json!({ "title": "Docs", "mime": "text/html" })),
            }),
            json!({
                "content": "page text",
                "title": "Docs",
                "mime": "text/html"
            })
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prompt_kernel_detached_bash_completes_while_idle() {
        let cwd = std::env::current_dir().expect("cwd");
        let kernel = devo_core::tools::handlers::ensure_kernel(
            &None,
            &cwd,
            None,
            devo_protocol::CollaborationMode::Build,
        )
        .await
        .expect("kernel");
        let registry = devo_core::tools::handlers::build_registry_from_plan(&Default::default());
        let runtime = Arc::new(ToolRuntime::new_with_context(
            Arc::new(registry),
            permission_checker(),
            devo_core::tools::ToolRuntimeContext {
                session_id: "ses_cli_kernel_idle".into(),
                cwd,
                kernel: Some(Arc::clone(&kernel)),
                ..Default::default()
            },
        ));
        kernel
            .set_host_handler(Some(super::host_request_handler(runtime)))
            .await;
        let idle = super::idle_host_request_handler("ses_cli_kernel_idle".to_string());
        let completed = Arc::new(tokio::sync::Notify::new());
        let observed = Arc::clone(&completed);
        kernel
            .set_idle_bash_completion_handler(Some(Arc::new(move |id, data| {
                let idle = Arc::clone(&idle);
                let observed = Arc::clone(&observed);
                Box::pin(async move {
                    let (action, _) = devo_kernel::parse_host_request_data(&data);
                    let result = idle(id, data).await;
                    if action == "bash.completed" {
                        observed.notify_one();
                    }
                    result
                })
            })))
            .await;
        let start = kernel
            .execute("h = bash('sleep 0.2; printf detached-ok')")
            .await
            .expect("start cell");
        assert_eq!(start.status, "ok", "{start:?}");
        tokio::time::timeout(std::time::Duration::from_secs(10), completed.notified())
            .await
            .expect("idle completion");
        let cached = kernel
            .execute("import json; print(json.dumps([h.running, h.poll().exit_code, h.output()]))")
            .await
            .expect("cached cell");
        assert_eq!(cached.status, "ok", "{cached:?}");
        let value: serde_json::Value =
            serde_json::from_str(cached.stdout.trim()).expect("cached result");
        assert_eq!(value, json!([false, 0, "detached-ok"]));
        kernel.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn idle_prompt_host_rejects_starts_but_routes_scoped_controls() {
        let handler = super::idle_host_request_handler("ses_cli_idle".to_string());
        assert_eq!(
            handler(
                "start".to_string(),
                json!({"type":"bash.start", "command":"echo forbidden"})
            )
            .await,
            json!({"status":"error", "error":"idle host request denied: bash.start"})
        );
        assert_eq!(
            handler(
                "poll".to_string(),
                json!({"type":"bash.poll", "params":{"handle_id":"missing"}})
            )
            .await,
            json!({"status":"error", "error":"unknown bash handle"})
        );
        assert_eq!(
            handler("notice".to_string(), json!({"type":"bash.completed"})).await,
            json!({"status":"ok", "result":{"accepted":true}})
        );
    }

    #[tokio::test]
    async fn prompt_host_rejects_interactive_questions() {
        let runtime = ToolRuntime::new_without_permissions(Arc::new(ToolRegistry::new()));
        assert_eq!(
            dispatch_host_request(
                &runtime,
                "request-1".to_string(),
                json!({"type":"question"})
            )
            .await,
            json!({
                "status": "error",
                "error": "interactive user questions are unavailable in `devo prompt`"
            })
        );
    }
}
