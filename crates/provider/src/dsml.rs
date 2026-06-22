//! Normalizes DSML text tool calls emitted by DeepSeek-style models.
//!
//! Some providers stream tool calls as tagged text instead of structured
//! protocol items. This module keeps that compatibility shim isolated from the
//! provider adapters: local tool calls are converted to `ResponseContent`,
//! hosted tool calls are preserved as text so the provider can keep handling
//! server-side tools itself.

use std::borrow::Cow;
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
    hosted_tool_names: BTreeSet<&'static str>,
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

    fn tool_kind_for_call(&self, call: &ToolCall) -> DsmlToolKind {
        if let Some(kind) = call.kind {
            return kind;
        }
        if self.hosted_tool_names.contains(call.name.as_str())
            && !self.local_tool_names.contains(call.name.as_str())
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
    let model = model
        .trim()
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(model)
        .trim();
    model
        .as_bytes()
        .get(..b"deepseek-v4-".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"deepseek-v4-"))
}

fn hosted_tool_name(tool: &HostedToolDefinition) -> &'static str {
    match tool {
        HostedToolDefinition::WebSearch(_) => "web_search",
        HostedToolDefinition::WebFetch(_) => "web_fetch",
    }
}

#[derive(Debug)]
pub(crate) struct DsmlTextStreamFilter {
    pending: String,
}

impl DsmlTextStreamFilter {
    pub(crate) fn consume(&mut self, chunk: &str) -> Vec<String> {
        self.pending.push_str(chunk);
        let mut output = Vec::new();

        loop {
            if let Some(block) = find_next_tool_calls_block(&self.pending, 0) {
                let block_start = block.start;
                let block_len = block.end - block.start;
                if block_start > 0 {
                    push_non_empty_text(&mut output, take_prefix(&mut self.pending, block_start));
                }
                self.pending.drain(..block_len);
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
                if start > 0 {
                    push_non_empty_text(&mut output, take_prefix(&mut self.pending, start));
                }
                if !self.pending[end - start..].contains(syntax.tool_calls_close) {
                    break;
                }
            }

            if let Some(partial_start) = earliest_partial_start(&self.pending) {
                if partial_start > 0 {
                    push_non_empty_text(&mut output, take_prefix(&mut self.pending, partial_start));
                }
                break;
            }

            push_non_empty_text(&mut output, std::mem::take(&mut self.pending));
            break;
        }

        output
    }

    pub(crate) fn finish(&mut self) -> Vec<String> {
        let pending = std::mem::take(&mut self.pending);
        if let Some(block) = find_next_tool_calls_open(&pending, 0) {
            return non_empty_text(pending[..block.start].to_string());
        }
        non_empty_text(pending)
    }
}

fn take_prefix(text: &mut String, end: usize) -> String {
    let suffix = text.split_off(end);
    std::mem::replace(text, suffix)
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
    // DSML parsing is done while scanning streamed text, so keep the full tag
    // spellings static rather than rebuilding marker/name strings for each
    // search. The prefixes intentionally omit `>` because attributes follow.
    tool_calls_open: &'static str,
    tool_calls_close: &'static str,
    invoke_open_prefix: &'static str,
    invoke_close: &'static str,
    parameter_open_prefix: &'static str,
    parameter_close: &'static str,
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
        tool_calls_open: "<｜DSML｜tool_calls>",
        tool_calls_close: "</｜DSML｜tool_calls>",
        invoke_open_prefix: "<｜DSML｜invoke",
        invoke_close: "</｜DSML｜invoke>",
        parameter_open_prefix: "<｜DSML｜parameter",
        parameter_close: "</｜DSML｜parameter>",
    },
    DsmlSyntax {
        tool_calls_open: "<｜｜DSML｜｜tool_calls>",
        tool_calls_close: "</｜｜DSML｜｜tool_calls>",
        invoke_open_prefix: "<｜｜DSML｜｜invoke",
        invoke_close: "</｜｜DSML｜｜invoke>",
        parameter_open_prefix: "<｜｜DSML｜｜parameter",
        parameter_close: "</｜｜DSML｜｜parameter>",
    },
    DsmlSyntax {
        tool_calls_open: "<|DSML|tool_calls>",
        tool_calls_close: "</|DSML|tool_calls>",
        invoke_open_prefix: "<|DSML|invoke",
        invoke_close: "</|DSML|invoke>",
        parameter_open_prefix: "<|DSML|parameter",
        parameter_close: "</|DSML|parameter>",
    },
    DsmlSyntax {
        tool_calls_open: "<||DSML||tool_calls>",
        tool_calls_close: "</||DSML||tool_calls>",
        invoke_open_prefix: "<||DSML||invoke",
        invoke_close: "</||DSML||invoke>",
        parameter_open_prefix: "<||DSML||parameter",
        parameter_close: "</||DSML||parameter>",
    },
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
    let start = text[cursor..].find(syntax.tool_calls_open)? + cursor;
    Some(ToolCallsBlock {
        start,
        end: start + syntax.tool_calls_open.len(),
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
    let start = text[cursor..].find(syntax.tool_calls_open)? + cursor;
    let inner_start = start + syntax.tool_calls_open.len();
    let close_start = text[inner_start..].find(syntax.tool_calls_close)? + inner_start;
    let end = close_start + syntax.tool_calls_close.len();
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

    while let Some(start_offset) = inner[cursor..].find(syntax.invoke_open_prefix) {
        let start = cursor + start_offset;
        let tag_end = inner[start..].find('>')? + start + 1;
        let tag = &inner[start..tag_end];
        let name = xml_unescape(attribute_value(tag, "name")?).into_owned();
        let kind = parse_tool_kind_attribute(tag);
        let close_start = inner[tag_end..].find(syntax.invoke_close)? + tag_end;
        let invoke_inner = &inner[tag_end..close_start];
        let input = parse_parameters(invoke_inner, syntax)?;

        calls.push(ToolCall { name, kind, input });
        cursor = close_start + syntax.invoke_close.len();
    }

    Some(calls)
}

fn parse_tool_kind_attribute(tag: &str) -> Option<DsmlToolKind> {
    let value = attribute_value(tag, "type").or_else(|| attribute_value(tag, "kind"))?;
    let value = value.trim();
    if value.eq_ignore_ascii_case("tool_use") || value.eq_ignore_ascii_case("local") {
        Some(DsmlToolKind::Local)
    } else if value.eq_ignore_ascii_case("server_tool_use")
        || value.eq_ignore_ascii_case("hosted_tool_use")
        || value.eq_ignore_ascii_case("hosted")
    {
        Some(DsmlToolKind::Hosted)
    } else {
        None
    }
}

fn parse_parameters(inner: &str, syntax: DsmlSyntax) -> Option<Map<String, Value>> {
    let mut input = Map::new();
    let mut cursor = 0usize;

    while let Some(start_offset) = inner[cursor..].find(syntax.parameter_open_prefix) {
        let start = cursor + start_offset;
        let tag_end = inner[start..].find('>')? + start + 1;
        let tag = &inner[start..tag_end];
        let name = xml_unescape(attribute_value(tag, "name")?).into_owned();
        let is_string = match attribute_value(tag, "string")? {
            "true" => true,
            "false" => false,
            _ => return None,
        };
        let close_start = inner[tag_end..].find(syntax.parameter_close)? + tag_end;
        let raw = &inner[tag_end..close_start];
        let value = if is_string {
            Value::String(xml_unescape(raw).into_owned())
        } else {
            serde_json::from_str(raw.trim()).ok()?
        };
        input.insert(name, value);
        cursor = close_start + syntax.parameter_close.len();
    }

    Some(input)
}

fn attribute_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    for quote in ['"', '\''] {
        if let Some(value_start) = tag.match_indices(name).find_map(|(index, value)| {
            let prefix = &tag[..index];
            if !prefix
                .chars()
                .next_back()
                .is_none_or(|ch| ch.is_ascii_whitespace() || ch == '<')
            {
                return None;
            }

            let value_start = index + value.len();
            let suffix = tag[value_start..].trim_start();
            let suffix = suffix.strip_prefix('=')?.trim_start();
            let suffix = suffix.strip_prefix(quote)?;
            Some(tag.len() - suffix.len())
        }) {
            let value_end = tag[value_start..].find(quote)? + value_start;
            return Some(&tag[value_start..value_end]);
        }
    }
    None
}

fn xml_unescape(text: &str) -> Cow<'_, str> {
    if !text.as_bytes().contains(&b'&') {
        return Cow::Borrowed(text);
    }

    let mut output = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find('&') {
        let start = cursor + offset;
        output.push_str(&text[cursor..start]);
        let remaining = &text[start..];
        let Some((decoded, len)) = xml_entity(remaining) else {
            output.push('&');
            cursor = start + 1;
            continue;
        };
        output.push(decoded);
        cursor = start + len;
    }
    output.push_str(&text[cursor..]);
    Cow::Owned(output)
}

fn xml_entity(text: &str) -> Option<(char, usize)> {
    XML_ENTITIES
        .iter()
        .find_map(|(entity, ch)| text.starts_with(entity).then_some((*ch, entity.len())))
}

const XML_ENTITIES: [(&str, char); 5] = [
    ("&quot;", '"'),
    ("&apos;", '\''),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&amp;", '&'),
];

fn earliest_partial_start(text: &str) -> Option<usize> {
    for (start, _) in text.char_indices() {
        let suffix = &text[start..];
        if SYNTAXES
            .iter()
            .any(|syntax| is_partial_marker(suffix, syntax.tool_calls_open))
        {
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
            request_thinking: None,
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
        assert!(
            DsmlToolCallHealer::for_model(" DeepSeek-V4-Pro ")
                .text_stream_filter()
                .is_some()
        );
        assert!(
            DsmlToolCallHealer::for_model("deepseek/deepseek-v4-flash")
                .text_stream_filter()
                .is_some()
        );
    }

    #[test]
    fn normalize_response_content_accepts_case_insensitive_kind_attribute() {
        let text = r#"<｜DSML｜tool_calls>
<｜DSML｜invoke name="web_search" kind="LOCAL">
<｜DSML｜parameter name="query" string="true">DeepSeek V4</｜DSML｜parameter>
</｜DSML｜invoke>
</｜DSML｜tool_calls>"#;

        let normalized = DsmlToolCallHealer::for_model("deepseek-v4-flash")
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
    fn normalize_response_content_reads_exact_attribute_names() {
        let text = r#"<｜DSML｜tool_calls>
<｜DSML｜invoke tool_name="wrong_tool" name="web_search">
<｜DSML｜parameter display_name="wrong_param" name="query" string="true">DeepSeek V4</｜DSML｜parameter>
</｜DSML｜invoke>
</｜DSML｜tool_calls>"#;

        let normalized = DsmlToolCallHealer::for_model("deepseek-v4-flash")
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
    fn normalize_response_content_accepts_attribute_whitespace() {
        let text = r#"<｜DSML｜tool_calls>
<｜DSML｜invoke name = "web_search">
<｜DSML｜parameter name = "query" string = "true">DeepSeek V4</｜DSML｜parameter>
</｜DSML｜invoke>
</｜DSML｜tool_calls>"#;

        let normalized = DsmlToolCallHealer::for_model("deepseek-v4-flash")
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
    fn stream_filter_suppresses_hosted_only_dsml_split_across_chunks() {
        let request = model_request_with_tools(
            "deepseek-v4-pro",
            /*tools*/ None,
            vec![HostedToolDefinition::WebSearch(HostedWebSearchTool::new())],
        );
        let mut filter = DsmlToolCallHealer::for_request(&request)
            .text_stream_filter()
            .expect("filter enabled");
        let mut emitted = Vec::new();

        emitted.extend(filter.consume("before<｜DS"));
        emitted.extend(filter.consume("ML｜tool_calls>\n<｜DSML｜invoke name=\"web_search\">\n"));
        emitted.extend(filter.consume("<｜DSML｜parameter name=\"query\" string=\"true\">DeepSeek V4</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>after"));
        emitted.extend(filter.finish());

        assert_eq!(emitted, vec!["before".to_string(), "after".to_string()]);
    }

    #[test]
    fn stream_filter_drops_unclosed_dsml_on_finish() {
        let request = model_request_with_tools(
            "deepseek-v4-pro",
            /*tools*/ None,
            vec![HostedToolDefinition::WebSearch(HostedWebSearchTool::new())],
        );
        let mut filter = DsmlToolCallHealer::for_request(&request)
            .text_stream_filter()
            .expect("filter enabled");
        let mut emitted = Vec::new();

        emitted.extend(filter.consume("visible<｜｜DSML｜｜tool_calls>\n"));
        emitted.extend(filter.consume("<｜｜DSML｜｜invoke name=\"web_search\">"));
        emitted.extend(filter.finish());

        assert_eq!(emitted, vec!["visible".to_string()]);
    }
}
