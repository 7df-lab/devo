use std::sync::Arc;

use devo_core::tools::{ToolCall, ToolContent, ToolRuntime};
use devo_kernel::HostRequestHandler;
use serde_json::{Map, Value, json};

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

    use devo_core::tools::{ToolRegistry, ToolRuntime};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::{content_as_json, dispatch_host_request};

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
