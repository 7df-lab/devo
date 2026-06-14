use std::collections::BTreeSet;

use devo_protocol::HostedToolDefinition;
use devo_protocol::ModelRequest;
use devo_protocol::ResponseContent;
use serde_json::Map;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub(crate) struct DsmlToolCallHealer {
    enabled: bool,
    local_tool_names: BTreeSet<String>,
    hosted_tool_names: BTreeSet<String>,
}

impl DsmlToolCallHealer {
    pub(crate) fn for_model(model: &str) -> Self {
        Self {
            enabled: model_uses_text_tool_calls(model),
            local_tool_names: BTreeSet::new(),
            hosted_tool_names: BTreeSet::new(),
        }
    }

    pub(crate) fn for_request(request: &ModelRequest) -> Self {
        let mut healer = Self::for_model(&request.model);
        if !healer.enabled {
            return healer;
        }

        if let Some(tools) = &request.tools {
            healer
                .local_tool_names
                .extend(tools.iter().map(|tool| tool.name.clone()));
        }
        healer
            .hosted_tool_names
            .extend(request.hosted_tools.iter().map(hosted_tool_name));

        healer
    }

    pub(crate) fn heal_response_content(
        &self,
        content: Vec<ResponseContent>,
    ) -> Vec<ResponseContent> {
        if !self.enabled {
            return content;
        }

        let mut output = Vec::new();
        let mut next_call_index = 0usize;
        for (block_index, block) in content.into_iter().enumerate() {
            match block {
                ResponseContent::Text(text) => {
                    match self.parse_text_segments(&text, block_index, &mut next_call_index) {
                        Some(segments) => output.extend(segments),
                        None => output.push(ResponseContent::Text(text)),
                    }
                }
                other => output.push(other),
            }
        }
        output
    }

    pub(crate) fn text_stream_filter(&self) -> Option<DsmlTextStreamFilter> {
        self.enabled.then(|| DsmlTextStreamFilter {
            healer: self.clone(),
            pending: String::new(),
        })
    }

    fn response_content_for_call(
        &self,
        block_index: usize,
        call_index: usize,
        call: ToolCall,
    ) -> ResponseContent {
        let id = format!("dsml_{block_index}_{call_index}");
        ResponseContent::ToolUse {
            id,
            name: call.name,
            input: Value::Object(call.input),
        }
    }

    fn tool_calls_block_is_local_only(&self, inner: &str, syntax: DsmlSyntax) -> bool {
        let Some(calls) = parse_tool_calls_block(inner, syntax) else {
            return false;
        };
        !calls.is_empty()
            && calls
                .iter()
                .all(|call| matches!(self.tool_kind_for_call(call), DsmlToolKind::Local))
    }

    fn tool_kind_for_call(&self, call: &ToolCall) -> DsmlToolKind {
        if let Some(kind) = call.kind {
            return kind;
        }
        if self.hosted_tool_names.contains(&call.name)
            && !self.local_tool_names.contains(&call.name)
        {
            return DsmlToolKind::Hosted;
        }
        DsmlToolKind::Local
    }

    fn parse_text_segments(
        &self,
        text: &str,
        block_index: usize,
        next_call_index: &mut usize,
    ) -> Option<Vec<ResponseContent>> {
        let mut output = Vec::new();
        let mut cursor = 0usize;
        let mut parsed_any = false;

        while let Some(block) = find_next_tool_calls_block(text, cursor) {
            if block.start > cursor {
                push_text_segment(&mut output, &text[cursor..block.start]);
            }

            let calls = parse_tool_calls_block(block.inner, block.syntax)?;
            if calls.is_empty() {
                return None;
            }
            if calls
                .iter()
                .any(|call| matches!(self.tool_kind_for_call(call), DsmlToolKind::Hosted))
            {
                push_text_segment(&mut output, &text[block.start..block.end]);
                parsed_any = true;
                cursor = block.end;
                continue;
            }

            parsed_any = true;
            for call in calls {
                let call_index = *next_call_index;
                output.push(self.response_content_for_call(block_index, call_index, call));
                *next_call_index += 1;
            }

            cursor = block.end;
        }

        if !parsed_any {
            return None;
        }
        if cursor < text.len() {
            push_text_segment(&mut output, &text[cursor..]);
        }
        Some(output)
    }
}

fn model_uses_text_tool_calls(model: &str) -> bool {
    model
        .trim()
        .to_ascii_lowercase()
        .starts_with("deepseek-v4-")
}

fn hosted_tool_name(tool: &HostedToolDefinition) -> String {
    match tool {
        HostedToolDefinition::WebSearch(_) => "web_search".to_string(),
        HostedToolDefinition::WebFetch(_) => "web_fetch".to_string(),
    }
}

#[derive(Debug)]
pub(crate) struct DsmlTextStreamFilter {
    healer: DsmlToolCallHealer,
    pending: String,
}

impl DsmlTextStreamFilter {
    pub(crate) fn consume(&mut self, chunk: &str) -> Vec<String> {
        self.pending.push_str(chunk);
        let mut output = Vec::new();

        loop {
            if let Some(block) = find_next_tool_calls_block(&self.pending, 0) {
                push_non_empty_text(&mut output, self.pending[..block.start].to_string());
                let should_suppress = self
                    .healer
                    .tool_calls_block_is_local_only(block.inner, block.syntax);
                if !should_suppress {
                    push_non_empty_text(
                        &mut output,
                        self.pending[block.start..block.end].to_string(),
                    );
                }
                self.pending.drain(..block.end);
                continue;
            }

            if let Some((start, end, syntax)) =
                find_next_tool_calls_open(&self.pending, 0).map(|block| {
                    let ToolCallsBlock {
                        start, end, syntax, ..
                    } = block;
                    (start, end, syntax)
                })
            {
                push_non_empty_text(&mut output, self.pending[..start].to_string());
                self.pending.drain(..start);
                let close = syntax.close_tag("tool_calls");
                if !self.pending[end - start..].contains(&close) {
                    break;
                }
            }

            if let Some(partial_start) = earliest_partial_start(&self.pending) {
                push_non_empty_text(&mut output, self.pending[..partial_start].to_string());
                self.pending.drain(..partial_start);
                break;
            }

            push_non_empty_text(&mut output, std::mem::take(&mut self.pending));
            break;
        }

        output
    }

    pub(crate) fn finish(&mut self) -> Vec<String> {
        non_empty_text(std::mem::take(&mut self.pending))
    }
}

fn non_empty_text(text: String) -> Vec<String> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![text]
    }
}

fn push_non_empty_text(output: &mut Vec<String>, text: String) {
    if !text.is_empty() {
        output.push(text);
    }
}

fn push_text_segment(output: &mut Vec<ResponseContent>, text: &str) {
    if !text.is_empty() {
        output.push(ResponseContent::Text(text.to_string()));
    }
}

#[derive(Clone, Copy, Debug)]
struct DsmlSyntax {
    marker: &'static str,
}

impl DsmlSyntax {
    fn open_tag(self, name: &str) -> String {
        format!("<{}{name}>", self.marker)
    }

    fn open_tag_prefix(self, name: &str) -> String {
        format!("<{}{name}", self.marker)
    }

    fn close_tag(self, name: &str) -> String {
        format!("</{}{name}>", self.marker)
    }
}

struct ToolCallsBlock<'a> {
    start: usize,
    end: usize,
    inner: &'a str,
    syntax: DsmlSyntax,
}

struct ToolCall {
    name: String,
    kind: Option<DsmlToolKind>,
    input: Map<String, Value>,
}

#[derive(Clone, Copy, Debug)]
enum DsmlToolKind {
    Local,
    Hosted,
}

const SYNTAXES: [DsmlSyntax; 4] = [
    DsmlSyntax {
        marker: "｜DSML｜"
    },
    DsmlSyntax {
        marker: "｜｜DSML｜｜",
    },
    DsmlSyntax { marker: "|DSML|" },
    DsmlSyntax { marker: "||DSML||" },
];

fn find_next_tool_calls_open(text: &str, cursor: usize) -> Option<ToolCallsBlock<'_>> {
    SYNTAXES
        .iter()
        .filter_map(|syntax| find_tool_calls_open_for_syntax(text, cursor, *syntax))
        .min_by_key(|block| block.start)
}

fn find_tool_calls_open_for_syntax(
    text: &str,
    cursor: usize,
    syntax: DsmlSyntax,
) -> Option<ToolCallsBlock<'_>> {
    let open = syntax.open_tag("tool_calls");
    let start = text[cursor..].find(&open)? + cursor;
    Some(ToolCallsBlock {
        start,
        end: start + open.len(),
        inner: "",
        syntax,
    })
}

fn find_next_tool_calls_block(text: &str, cursor: usize) -> Option<ToolCallsBlock<'_>> {
    SYNTAXES
        .iter()
        .filter_map(|syntax| find_tool_calls_block_for_syntax(text, cursor, *syntax))
        .min_by_key(|block| block.start)
}

fn find_tool_calls_block_for_syntax(
    text: &str,
    cursor: usize,
    syntax: DsmlSyntax,
) -> Option<ToolCallsBlock<'_>> {
    let open = syntax.open_tag("tool_calls");
    let close = syntax.close_tag("tool_calls");
    let start = text[cursor..].find(&open)? + cursor;
    let inner_start = start + open.len();
    let close_start = text[inner_start..].find(&close)? + inner_start;
    let end = close_start + close.len();
    Some(ToolCallsBlock {
        start,
        end,
        inner: &text[inner_start..close_start],
        syntax,
    })
}

fn parse_tool_calls_block(inner: &str, syntax: DsmlSyntax) -> Option<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut cursor = 0usize;
    let invoke_prefix = syntax.open_tag_prefix("invoke");
    let invoke_close = syntax.close_tag("invoke");

    while let Some(start_offset) = inner[cursor..].find(&invoke_prefix) {
        let start = cursor + start_offset;
        let tag_end = inner[start..].find('>')? + start + 1;
        let tag = &inner[start..tag_end];
        let name = xml_unescape(&attribute_value(tag, "name")?);
        let kind = parse_tool_kind_attribute(tag);
        let close_start = inner[tag_end..].find(&invoke_close)? + tag_end;
        let invoke_inner = &inner[tag_end..close_start];
        let input = parse_parameters(invoke_inner, syntax)?;

        calls.push(ToolCall { name, kind, input });
        cursor = close_start + invoke_close.len();
    }

    Some(calls)
}

fn parse_tool_kind_attribute(tag: &str) -> Option<DsmlToolKind> {
    let value = attribute_value(tag, "type").or_else(|| attribute_value(tag, "kind"))?;
    match value.trim().to_ascii_lowercase().as_str() {
        "tool_use" | "local" => Some(DsmlToolKind::Local),
        "server_tool_use" | "hosted_tool_use" | "hosted" => Some(DsmlToolKind::Hosted),
        _ => None,
    }
}

fn parse_parameters(inner: &str, syntax: DsmlSyntax) -> Option<Map<String, Value>> {
    let mut input = Map::new();
    let mut cursor = 0usize;
    let parameter_prefix = syntax.open_tag_prefix("parameter");
    let parameter_close = syntax.close_tag("parameter");

    while let Some(start_offset) = inner[cursor..].find(&parameter_prefix) {
        let start = cursor + start_offset;
        let tag_end = inner[start..].find('>')? + start + 1;
        let tag = &inner[start..tag_end];
        let name = xml_unescape(&attribute_value(tag, "name")?);
        let is_string = match attribute_value(tag, "string")?.as_str() {
            "true" => true,
            "false" => false,
            _ => return None,
        };
        let close_start = inner[tag_end..].find(&parameter_close)? + tag_end;
        let raw = &inner[tag_end..close_start];
        let value = if is_string {
            Value::String(xml_unescape(raw))
        } else {
            serde_json::from_str(raw.trim()).ok()?
        };
        input.insert(name, value);
        cursor = close_start + parameter_close.len();
    }

    Some(input)
}

fn attribute_value(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let pattern = format!("{name}={quote}");
        if let Some(value_start) = tag.find(&pattern).map(|index| index + pattern.len()) {
            let value_end = tag[value_start..].find(quote)? + value_start;
            return Some(tag[value_start..value_end].to_string());
        }
    }
    None
}

fn xml_unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn earliest_partial_start(text: &str) -> Option<usize> {
    for (start, _) in text.char_indices() {
        let suffix = &text[start..];
        if SYNTAXES.iter().any(|syntax| {
            let open = syntax.open_tag("tool_calls");
            is_partial_marker(suffix, &open)
        }) {
            return Some(start);
        }
    }
    None
}

fn is_partial_marker(text: &str, marker: &str) -> bool {
    marker.starts_with(text) && !text.is_empty() && text.len() < marker.len()
}

#[cfg(test)]
mod tests {
    use devo_protocol::HostedToolDefinition;
    use devo_protocol::HostedWebSearchTool;
    use devo_protocol::ModelRequest;
    use devo_protocol::SamplingControls;
    use devo_protocol::ToolDefinition;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    fn model_request_with_tools(
        model: &str,
        tools: Option<Vec<ToolDefinition>>,
        hosted_tools: Vec<HostedToolDefinition>,
    ) -> ModelRequest {
        ModelRequest {
            model: model.to_string(),
            system: None,
            messages: Vec::new(),
            max_tokens: 1024,
            tools,
            hosted_tools,
            sampling: SamplingControls::default(),
            thinking: None,
            reasoning_effort: None,
            extra_body: None,
        }
    }

    #[test]
    fn normalize_response_content_extracts_dsml_tool_use_for_deepseek_v4() {
        let text = r#"before
<｜｜DSML｜｜tool_calls>
<｜｜DSML｜｜invoke name="web_search">
<｜｜DSML｜｜parameter name="query" string="true">electron &quot;vite&quot;</｜｜DSML｜｜parameter>
<｜｜DSML｜｜parameter name="limit" string="false">5</｜｜DSML｜｜parameter>
<｜｜DSML｜｜parameter name="fresh" string="false">true</｜｜DSML｜｜parameter>
</｜｜DSML｜｜invoke>
</｜｜DSML｜｜tool_calls>
after"#;

        let healer = DsmlToolCallHealer::for_model("deepseek-v4-pro");
        let normalized = healer.heal_response_content(vec![ResponseContent::Text(text.into())]);

        assert_eq!(
            normalized,
            vec![
                ResponseContent::Text("before\n".to_string()),
                ResponseContent::ToolUse {
                    id: "dsml_0_0".to_string(),
                    name: "web_search".to_string(),
                    input: json!({
                        "query": "electron \"vite\"",
                        "limit": 5,
                        "fresh": true
                    }),
                },
                ResponseContent::Text("\nafter".to_string()),
            ]
        );
    }

    #[test]
    fn normalize_response_content_preserves_hosted_tool_use_from_request_context() {
        let text = r#"<｜DSML｜tool_calls>
<｜DSML｜invoke name="web_search">
<｜DSML｜parameter name="query" string="true">DeepSeek V4</｜DSML｜parameter>
</｜DSML｜invoke>
</｜DSML｜tool_calls>"#;
        let request = model_request_with_tools(
            "deepseek-v4-pro",
            /*tools*/ None,
            vec![HostedToolDefinition::WebSearch(HostedWebSearchTool::new())],
        );

        let normalized = DsmlToolCallHealer::for_request(&request)
            .heal_response_content(vec![ResponseContent::Text(text.to_string())]);

        assert_eq!(normalized, vec![ResponseContent::Text(text.to_string())]);
    }

    #[test]
    fn normalize_response_content_prefers_local_tool_when_name_is_ambiguous() {
        let text = r#"<｜DSML｜tool_calls>
<｜DSML｜invoke name="web_search">
<｜DSML｜parameter name="query" string="true">DeepSeek V4</｜DSML｜parameter>
</｜DSML｜invoke>
</｜DSML｜tool_calls>"#;
        let request = model_request_with_tools(
            "deepseek-v4-pro",
            Some(vec![ToolDefinition {
                name: "web_search".to_string(),
                description: "Local search implementation".to_string(),
                input_schema: json!({ "type": "object" }),
                output_schema: None,
            }]),
            vec![HostedToolDefinition::WebSearch(HostedWebSearchTool::new())],
        );

        let normalized = DsmlToolCallHealer::for_request(&request)
            .heal_response_content(vec![ResponseContent::Text(text.to_string())]);

        assert_eq!(
            normalized,
            vec![ResponseContent::ToolUse {
                id: "dsml_0_0".to_string(),
                name: "web_search".to_string(),
                input: json!({"query": "DeepSeek V4"}),
            }]
        );
    }

    #[test]
    fn normalize_response_content_preserves_explicit_server_tool_use_kind() {
        let text = r#"<｜DSML｜tool_calls>
<｜DSML｜invoke name="web_search" type="server_tool_use">
<｜DSML｜parameter name="query" string="true">DeepSeek V4</｜DSML｜parameter>
</｜DSML｜invoke>
</｜DSML｜tool_calls>"#;

        let normalized = DsmlToolCallHealer::for_model("deepseek-v4-pro")
            .heal_response_content(vec![ResponseContent::Text(text.to_string())]);

        assert_eq!(normalized, vec![ResponseContent::Text(text.to_string())]);
    }

    #[test]
    fn normalize_response_content_is_gated_to_deepseek_v4_models() {
        let text = "<｜DSML｜tool_calls></｜DSML｜tool_calls>".to_string();
        let content = vec![ResponseContent::Text(text.clone())];

        assert_eq!(
            DsmlToolCallHealer::for_model("claude-sonnet").heal_response_content(content.clone()),
            content
        );
        assert_eq!(
            DsmlToolCallHealer::for_model("deepseek-v3").heal_response_content(content.clone()),
            content
        );
        assert!(
            DsmlToolCallHealer::for_model("deepseek-v4-pro")
                .text_stream_filter()
                .is_some()
        );
    }

    #[test]
    fn normalize_response_content_preserves_text_when_structured_json_is_invalid() {
        let text = r#"<｜DSML｜tool_calls>
<｜DSML｜invoke name="web_search">
<｜DSML｜parameter name="limit" string="false">not-json</｜DSML｜parameter>
</｜DSML｜invoke>
</｜DSML｜tool_calls>"#;
        let content = vec![ResponseContent::Text(text.to_string())];

        assert_eq!(
            DsmlToolCallHealer::for_model("deepseek-v4-flash")
                .heal_response_content(content.clone()),
            content
        );
    }

    #[test]
    fn stream_filter_suppresses_dsml_split_across_chunks() {
        let mut filter = DsmlToolCallHealer::for_model("deepseek-v4-pro")
            .text_stream_filter()
            .expect("filter enabled");
        let mut emitted = Vec::new();

        emitted.extend(filter.consume("before<｜｜DS"));
        emitted.extend(filter.consume("ML｜｜tool_calls>"));
        emitted.extend(filter.consume(
            "<｜｜DSML｜｜invoke name=\"read\"><｜｜DSML｜｜parameter name=\"path\" string=\"true\">README.md</｜｜DSML｜｜parameter></｜｜DSML｜｜invoke></｜｜DSML｜｜tool_calls>after",
        ));
        emitted.extend(filter.finish());

        assert_eq!(emitted, vec!["before".to_string(), "after".to_string()]);
    }

    #[test]
    fn stream_filter_preserves_hosted_only_dsml_split_across_chunks() {
        let request = model_request_with_tools(
            "deepseek-v4-pro",
            /*tools*/ None,
            vec![HostedToolDefinition::WebSearch(HostedWebSearchTool::new())],
        );
        let mut filter = DsmlToolCallHealer::for_request(&request)
            .text_stream_filter()
            .expect("filter enabled");
        let text = r#"before<｜DSML｜tool_calls>
<｜DSML｜invoke name="web_search">
<｜DSML｜parameter name="query" string="true">DeepSeek V4</｜DSML｜parameter>
</｜DSML｜invoke>
</｜DSML｜tool_calls>after"#;
        let mut emitted = Vec::new();

        emitted.extend(filter.consume("before<｜DS"));
        emitted.extend(filter.consume("ML｜tool_calls>\n<｜DSML｜invoke name=\"web_search\">\n"));
        emitted.extend(filter.consume("<｜DSML｜parameter name=\"query\" string=\"true\">DeepSeek V4</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>after"));
        emitted.extend(filter.finish());

        assert_eq!(emitted.concat(), text);
    }
}
