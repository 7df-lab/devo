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

async fn dispatch_host_request(runtime: &ToolRuntime, request_id: String, data: Value) -> Value {
    let Some(action) = data
        .get("type")
        .or_else(|| data.get("action"))
        .and_then(Value::as_str)
    else {
        return error_reply("host request is missing a type");
    };

    let (tool_name, input) = match action {
        "bash" | "bash.start" => {
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
        for action in ["bash", "bash.start"] {
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
