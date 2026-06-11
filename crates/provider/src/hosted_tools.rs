use devo_protocol::HostedToolDefinition;
use devo_protocol::HostedWebFetchTool;
use devo_protocol::HostedWebSearchTool;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

const DEFAULT_ANTHROPIC_WEB_SEARCH_TOOL_TYPE: &str = "web_search_20250305";
const DEFAULT_ANTHROPIC_WEB_FETCH_TOOL_TYPE: &str = "web_fetch_20250910";

pub(crate) fn append_openai_responses_hosted_tools(
    root: &mut Value,
    hosted_tools: &[HostedToolDefinition],
) {
    for hosted_tool in hosted_tools {
        match hosted_tool {
            HostedToolDefinition::WebSearch(options) => push_tool(root, openai_web_search(options)),
            HostedToolDefinition::WebFetch(options) => push_tool(root, openai_web_fetch(options)),
        }
    }
}

pub(crate) fn apply_openai_chat_completions_hosted_tools(
    root: &mut Value,
    hosted_tools: &[HostedToolDefinition],
) {
    for hosted_tool in hosted_tools {
        match hosted_tool {
            HostedToolDefinition::WebSearch(options) => {
                let mut value = Map::new();
                if let Some(search_context_size) = &options.search_context_size {
                    value.insert(
                        "search_context_size".to_string(),
                        json!(search_context_size),
                    );
                }
                root["web_search_options"] = Value::Object(value);
            }
            HostedToolDefinition::WebFetch(options) => {
                root["web_fetch_options"] = openai_web_fetch_options(options);
            }
        }
    }
}

pub(crate) fn append_anthropic_hosted_tools(
    root: &mut Value,
    hosted_tools: &[HostedToolDefinition],
) {
    for hosted_tool in hosted_tools {
        match hosted_tool {
            HostedToolDefinition::WebSearch(options) => {
                push_tool(root, anthropic_web_search(options));
            }
            HostedToolDefinition::WebFetch(options) => {
                push_tool(root, anthropic_web_fetch(options));
            }
        }
    }
}

fn openai_web_search(options: &HostedWebSearchTool) -> Value {
    let mut value = Map::from_iter([("type".to_string(), json!("web_search"))]);
    if let Some(search_context_size) = &options.search_context_size {
        value.insert(
            "search_context_size".to_string(),
            json!(search_context_size),
        );
    }
    Value::Object(value)
}

fn openai_web_fetch(options: &HostedWebFetchTool) -> Value {
    let mut value = Map::from_iter([("type".to_string(), json!("web_fetch"))]);
    append_web_fetch_common_options(&mut value, options);
    Value::Object(value)
}

fn openai_web_fetch_options(options: &HostedWebFetchTool) -> Value {
    let mut value = Map::new();
    append_web_fetch_common_options(&mut value, options);
    Value::Object(value)
}

fn anthropic_web_search(options: &HostedWebSearchTool) -> Value {
    let tool_type = options
        .anthropic_tool_type
        .as_deref()
        .unwrap_or(DEFAULT_ANTHROPIC_WEB_SEARCH_TOOL_TYPE);
    let mut value = Map::from_iter([
        ("type".to_string(), json!(tool_type)),
        ("name".to_string(), json!("web_search")),
    ]);
    if let Some(max_uses) = options.max_uses {
        value.insert("max_uses".to_string(), json!(max_uses));
    }
    Value::Object(value)
}

fn anthropic_web_fetch(options: &HostedWebFetchTool) -> Value {
    let tool_type = options
        .anthropic_tool_type
        .as_deref()
        .unwrap_or(DEFAULT_ANTHROPIC_WEB_FETCH_TOOL_TYPE);
    let mut value = Map::from_iter([
        ("type".to_string(), json!(tool_type)),
        ("name".to_string(), json!("web_fetch")),
    ]);
    append_web_fetch_common_options(&mut value, options);
    Value::Object(value)
}

fn append_web_fetch_common_options(value: &mut Map<String, Value>, options: &HostedWebFetchTool) {
    if let Some(max_uses) = options.max_uses {
        value.insert("max_uses".to_string(), json!(max_uses));
    }
    if !options.allowed_domains.is_empty() {
        value.insert(
            "allowed_domains".to_string(),
            json!(options.allowed_domains),
        );
    }
    if !options.blocked_domains.is_empty() {
        value.insert(
            "blocked_domains".to_string(),
            json!(options.blocked_domains),
        );
    }
    if let Some(citations) = options.citations {
        value.insert("citations".to_string(), json!(citations));
    }
    if let Some(max_content_tokens) = options.max_content_tokens {
        value.insert("max_content_tokens".to_string(), json!(max_content_tokens));
    }
}

fn push_tool(root: &mut Value, tool: Value) {
    match root.get_mut("tools").and_then(Value::as_array_mut) {
        Some(tools) => tools.push(tool),
        None => root["tools"] = Value::Array(vec![tool]),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    fn web_search_tools() -> Vec<HostedToolDefinition> {
        vec![HostedToolDefinition::WebSearch(HostedWebSearchTool::new())]
    }

    fn web_fetch_tools() -> Vec<HostedToolDefinition> {
        vec![HostedToolDefinition::WebFetch(HostedWebFetchTool {
            allowed_domains: vec!["docs.example".to_string()],
            citations: Some(true),
            ..HostedWebFetchTool::new()
        })]
    }

    #[test]
    fn openai_responses_appends_hosted_web_search_tool() {
        let mut root = json!({ "tools": [] });

        append_openai_responses_hosted_tools(&mut root, &web_search_tools());

        assert_eq!(root["tools"], json!([{ "type": "web_search" }]));
    }

    #[test]
    fn openai_chat_completions_adds_web_search_options_without_touching_model() {
        let mut root = json!({ "model": "vendor/search-capable-model" });

        apply_openai_chat_completions_hosted_tools(&mut root, &web_search_tools());

        assert_eq!(root["model"], json!("vendor/search-capable-model"));
        assert_eq!(root["web_search_options"], json!({}));
    }

    #[test]
    fn anthropic_messages_appends_server_web_search_tool() {
        let mut root = json!({});

        append_anthropic_hosted_tools(&mut root, &web_search_tools());

        assert_eq!(
            root["tools"],
            json!([{ "type": "web_search_20250305", "name": "web_search" }])
        );
    }

    #[test]
    fn openai_responses_appends_hosted_web_fetch_tool() {
        let mut root = json!({});

        append_openai_responses_hosted_tools(&mut root, &web_fetch_tools());

        assert_eq!(
            root["tools"],
            json!([{ "type": "web_fetch", "allowed_domains": ["docs.example"], "citations": true }])
        );
    }

    #[test]
    fn openai_chat_completions_adds_web_fetch_options() {
        let mut root = json!({});

        apply_openai_chat_completions_hosted_tools(&mut root, &web_fetch_tools());

        assert_eq!(
            root["web_fetch_options"],
            json!({ "allowed_domains": ["docs.example"], "citations": true })
        );
    }

    #[test]
    fn anthropic_messages_appends_server_web_fetch_tool() {
        let mut root = json!({});

        append_anthropic_hosted_tools(&mut root, &web_fetch_tools());

        assert_eq!(
            root["tools"],
            json!([{
                "type": "web_fetch_20250910",
                "name": "web_fetch",
                "allowed_domains": ["docs.example"],
                "citations": true
            }])
        );
    }
}
