use std::collections::BTreeMap;

use devo_protocol::{
    ModelRequest, ModelResponse, ResponseContent, ResponseExtra, ResponseMetadata, StopReason,
    StreamEvent, Usage,
};
use serde_json::Value;

use super::super::{
    OpenAIChatCompletionChoice, OpenAIChatCompletionCustomToolCall,
    OpenAIChatCompletionFunctionCall, OpenAIChatCompletionMessage,
    OpenAIChatCompletionMessageToolCall, OpenAIChatCompletionResponse, OpenAIChoiceLogprobs,
    build_provider_specific_response_payload, parse_finish_reason, parse_tool_use,
};
use super::{
    ChatCompletionStreamChoice, ChatCompletionStreamChunk, ChatCompletionStreamToolCallDelta,
    stop_reason_to_finish_reason,
};
use crate::dsml::{DsmlTextStreamFilter, DsmlToolCallHealer};
use crate::openai::shared::{
    OpenAICompletionTokenDetails, OpenAICompletionUsage, OpenAIPromptTokenDetails,
    completion_usage_to_usage,
};
use crate::text_normalization::{TaggedTextFragment, TaggedTextParser};

#[derive(Debug, Default)]
pub(super) struct ChatCompletionStreamState {
    response_id: String,
    created: Option<u64>,
    model: Option<String>,
    object: Option<String>,
    service_tier: Option<String>,
    system_fingerprint: Option<String>,
    text: StreamTextBlock,
    text_parser: TaggedTextParser,
    reasoning: StreamTextBlock,
    reasoning_content_active: bool,
    refusal: String,
    role: Option<String>,
    tool_calls: BTreeMap<u32, PartialToolCall>,
    choice_metadata: BTreeMap<u32, StreamChoiceMetadata>,
    finish_reason: Option<StopReason>,
    usage: Option<Usage>,
    dsml_healer: DsmlToolCallHealer,
    dsml_text_filter: Option<DsmlTextStreamFilter>,
}

impl ChatCompletionStreamState {
    pub(super) fn for_request(request: &ModelRequest) -> Self {
        Self::with_healer(DsmlToolCallHealer::for_request(request))
    }

    #[cfg(test)]
    pub(super) fn for_model(model: &str) -> Self {
        Self::with_healer(DsmlToolCallHealer::for_model(model))
    }

    fn with_healer(dsml_healer: DsmlToolCallHealer) -> Self {
        Self {
            dsml_text_filter: dsml_healer.text_stream_filter(),
            dsml_healer,
            ..Self::default()
        }
    }

    pub(super) fn apply_chunk(&mut self, chunk: ChatCompletionStreamChunk) -> Vec<StreamEvent> {
        let mut events = Vec::new();

        if self.response_id.is_empty()
            && let Some(id) = chunk.id
        {
            self.response_id = id;
        }
        if self.created.is_none() {
            self.created = chunk.created;
        }
        if self.model.is_none() {
            self.model = chunk.model;
        }
        if self.object.is_none() {
            self.object = chunk.object;
        }
        if self.service_tier.is_none() {
            self.service_tier = chunk.service_tier;
        }
        if self.system_fingerprint.is_none() {
            self.system_fingerprint = chunk.system_fingerprint;
        }

        if let Some(usage) = chunk.usage.as_ref().map(completion_usage_to_usage) {
            self.usage = Some(usage.clone());
            events.push(StreamEvent::UsageDelta(usage));
        }

        for choice in chunk.choices {
            self.apply_choice(choice, &mut events);
        }

        events
    }

    fn apply_choice(&mut self, choice: ChatCompletionStreamChoice, events: &mut Vec<StreamEvent>) {
        let choice_index = choice.index.unwrap_or(0);
        self.choice_metadata.entry(choice_index).or_default().index = Some(choice_index);
        self.choice_metadata
            .entry(choice_index)
            .or_default()
            .logprobs = choice.logprobs.clone();

        if let Some(role) = choice.delta.role.as_deref() {
            self.role = Some(role.to_string());
        }
        if let Some(refusal) = choice.delta.refusal.filter(|refusal| !refusal.is_empty()) {
            self.refusal.push_str(&refusal);
        }

        match choice.delta.reasoning_content {
            Some(reasoning_content) => {
                self.reasoning_content_active = true;
                if !reasoning_content.is_empty() {
                    self.push_reasoning_delta(reasoning_content, events);
                }
            }
            None if self.reasoning_content_active => {
                self.reasoning_content_active = false;
                self.push_reasoning_done(events);
            }
            None => {}
        }

        if let Some(content) = choice.delta.content.filter(|content| !content.is_empty()) {
            for fragment in self.text_parser.consume(&content) {
                match fragment {
                    TaggedTextFragment::Text(text) => self.push_text_delta(text, events),
                    TaggedTextFragment::Reasoning(text) => self.push_reasoning_delta(text, events),
                }
            }
        }

        for (fallback_index, tool_call_delta) in choice.delta.tool_calls.into_iter().enumerate() {
            let fallback_index = u32::try_from(fallback_index).unwrap_or(u32::MAX);
            self.apply_tool_call_delta(tool_call_delta, fallback_index, events);
        }

        if let Some(reason) = choice.finish_reason {
            self.finish_pending_content_into(events);
            self.finish_reason = Some(parse_finish_reason(&reason));
            self.choice_metadata
                .entry(choice_index)
                .or_default()
                .finish_reason = Some(reason);
        }
    }

    fn push_text_delta(&mut self, text: String, events: &mut Vec<StreamEvent>) {
        if text.is_empty() {
            return;
        }
        self.text.value.push_str(&text);
        let text_chunks = if let Some(filter) = &mut self.dsml_text_filter {
            filter.consume(&text)
        } else {
            vec![text]
        };
        self.push_visible_text_delta(text_chunks, events);
    }

    fn push_visible_text_delta(&mut self, text_chunks: Vec<String>, events: &mut Vec<StreamEvent>) {
        for text in text_chunks {
            if text.is_empty() {
                continue;
            }
            if !self.text.started {
                self.text.started = true;
                events.push(StreamEvent::TextStart { index: 0 });
            }
            events.push(StreamEvent::TextDelta { index: 0, text });
        }
    }

    pub(super) fn finish_pending_content(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        self.finish_pending_content_into(&mut events);
        events
    }

    fn finish_pending_content_into(&mut self, events: &mut Vec<StreamEvent>) {
        self.finish_tagged_text_into(events);
        self.finish_pending_text_into(events);
        self.finish_reasoning_into(events);
    }

    fn finish_tagged_text_into(&mut self, events: &mut Vec<StreamEvent>) {
        for fragment in self.text_parser.finish() {
            match fragment {
                TaggedTextFragment::Text(text) => self.push_text_delta(text, events),
                TaggedTextFragment::Reasoning(text) => self.push_reasoning_delta(text, events),
            }
        }
    }

    fn finish_pending_text_into(&mut self, events: &mut Vec<StreamEvent>) {
        let Some(filter) = &mut self.dsml_text_filter else {
            return;
        };
        let text_chunks = filter.finish();
        self.push_visible_text_delta(text_chunks, events);
    }

    fn push_reasoning_delta(&mut self, text: String, events: &mut Vec<StreamEvent>) {
        if text.is_empty() {
            return;
        }
        if !self.reasoning.started || self.reasoning.finished {
            self.reasoning.started = true;
            self.reasoning.finished = false;
            events.push(StreamEvent::ReasoningStart { index: 1 });
        }
        self.reasoning.value.push_str(&text);
        events.push(StreamEvent::ReasoningDelta { index: 1, text });
    }

    fn finish_reasoning_into(&mut self, events: &mut Vec<StreamEvent>) {
        self.reasoning_content_active = false;
        self.push_reasoning_done(events);
    }

    fn push_reasoning_done(&mut self, events: &mut Vec<StreamEvent>) {
        if self.reasoning.started && !self.reasoning.finished {
            self.reasoning.finished = true;
            events.push(StreamEvent::ReasoningDone { index: 1 });
        }
    }

    fn apply_tool_call_delta(
        &mut self,
        tool_call_delta: ChatCompletionStreamToolCallDelta,
        fallback_index: u32,
        events: &mut Vec<StreamEvent>,
    ) {
        let ChatCompletionStreamToolCallDelta {
            index,
            id,
            kind,
            function,
            custom,
        } = tool_call_delta;
        let index = index.unwrap_or(fallback_index);
        let content_index =
            usize::try_from(index).map_or(usize::MAX, |index| index.saturating_add(1));
        let entry = self.tool_calls.entry(index).or_default();

        if let Some(id) = id {
            entry.id = id;
        }
        if let Some(kind) = kind.as_deref().and_then(ToolCallKind::from_wire) {
            entry.kind = kind;
        }

        let mut function_arguments_delta = None;
        if let Some(function) = function {
            if let Some(name) = function.name {
                entry.name = name;
            }

            if let Some(arguments) = function.arguments.filter(|arguments| !arguments.is_empty()) {
                function_arguments_delta = Some(arguments.clone());
                entry.arguments_json.push_str(&arguments);
            }
        }

        let mut custom_input_delta = None;
        if let Some(custom) = custom {
            if let Some(name) = custom.name {
                entry.name = name;
            }
            if let Some(input) = custom.input.filter(|input| !input.is_empty()) {
                custom_input_delta = Some(input.clone());
                entry.arguments_json.push_str(&input);
            }
        }

        let newly_started = if !entry.started && tool_call_delta_starts_block(entry) {
            entry.started = true;
            events.push(StreamEvent::ToolCallStart {
                index: content_index,
                id: entry.id.clone(),
                name: entry.name.clone(),
                input: entry.kind.empty_input(),
            });
            true
        } else {
            false
        };

        if newly_started {
            if !entry.arguments_json.is_empty() {
                events.push(StreamEvent::ToolCallInputDelta {
                    index: content_index,
                    partial_json: entry.arguments_json.clone(),
                });
            }
            return;
        }

        if entry.started
            && let Some(arguments) = function_arguments_delta
        {
            events.push(StreamEvent::ToolCallInputDelta {
                index: content_index,
                partial_json: arguments,
            });
        }
        if entry.started
            && let Some(input) = custom_input_delta
        {
            events.push(StreamEvent::ToolCallInputDelta {
                index: content_index,
                partial_json: input,
            });
        }
    }

    fn metadata(&self) -> ResponseMetadata {
        let mut metadata = ResponseMetadata::default();

        if !self.reasoning.value.is_empty() {
            metadata.extras.push(ResponseExtra::ReasoningText {
                text: self.reasoning.value.clone(),
            });
        }

        if let Some(payload) = self.provider_specific_payload() {
            metadata.extras.push(ResponseExtra::ProviderSpecific {
                provider: "openai".to_string(),
                payload,
            });
        }

        metadata
    }

    fn provider_specific_payload(&self) -> Option<Value> {
        let response = self.as_provider_response();
        build_provider_specific_response_payload(&response)
    }

    fn as_provider_response(&self) -> OpenAIChatCompletionResponse {
        OpenAIChatCompletionResponse {
            id: self.response_id.clone(),
            choices: self.provider_choices(),
            created: self.created,
            model: self.model.clone(),
            object: self.object.clone(),
            service_tier: self.service_tier.clone(),
            system_fingerprint: self.system_fingerprint.clone(),
            usage: self.usage.as_ref().map(|usage| OpenAICompletionUsage {
                prompt_tokens: usage.input_tokens,
                completion_tokens: usage.output_tokens,
                total_tokens: usage.total_tokens,
                prompt_tokens_details: Some(OpenAIPromptTokenDetails {
                    audio_tokens: None,
                    cached_tokens: usage.cache_read_input_tokens,
                }),
                completion_tokens_details: usage.reasoning_output_tokens.map(|reasoning_tokens| {
                    OpenAICompletionTokenDetails {
                        accepted_prediction_tokens: None,
                        audio_tokens: None,
                        reasoning_tokens: Some(reasoning_tokens),
                        rejected_prediction_tokens: None,
                    }
                }),
            }),
        }
    }

    fn provider_choices(&self) -> Vec<OpenAIChatCompletionChoice> {
        let message = OpenAIChatCompletionMessage {
            content: if self.text.value.is_empty() {
                None
            } else {
                Some(self.text.value.clone())
            },
            refusal: if self.refusal.is_empty() {
                None
            } else {
                Some(self.refusal.clone())
            },
            role: self.role.clone(),
            annotations: Vec::new(),
            audio: None,
            function_call: None,
            tool_calls: self.provider_tool_calls(),
            reasoning_content: if self.reasoning.value.is_empty() {
                None
            } else {
                Some(self.reasoning.value.clone())
            },
        };

        if self.choice_metadata.is_empty()
            && message.content.is_none()
            && message.refusal.is_none()
            && message.tool_calls.is_empty()
            && self.finish_reason.is_none()
        {
            return Vec::new();
        }

        if self.choice_metadata.is_empty() {
            return vec![OpenAIChatCompletionChoice {
                finish_reason: self
                    .finish_reason
                    .as_ref()
                    .map(stop_reason_to_finish_reason),
                index: Some(0),
                logprobs: None,
                message: Some(message),
            }];
        }

        self.choice_metadata
            .iter()
            .map(|(index, metadata)| OpenAIChatCompletionChoice {
                finish_reason: metadata.finish_reason.clone().or_else(|| {
                    self.finish_reason
                        .as_ref()
                        .map(stop_reason_to_finish_reason)
                }),
                index: Some(*index),
                logprobs: metadata.logprobs.clone(),
                message: Some(message.clone()),
            })
            .collect()
    }

    fn provider_tool_calls(&self) -> Vec<OpenAIChatCompletionMessageToolCall> {
        self.tool_calls
            .values()
            .cloned()
            .map(PartialToolCall::into_provider_tool_call)
            .collect()
    }

    fn ordered_content(
        text: StreamTextBlock,
        tool_calls: BTreeMap<u32, PartialToolCall>,
    ) -> Vec<ResponseContent> {
        let mut content = Vec::new();

        if !text.value.is_empty() {
            content.push(ResponseContent::Text(text.value));
        }

        for tool_call in tool_calls.into_values() {
            let provider_tool_call = tool_call.into_provider_tool_call();
            if let Some(content_block) = parse_tool_use(&provider_tool_call) {
                content.push(content_block);
            }
        }

        content
    }

    pub(super) fn into_response(self) -> ModelResponse {
        let usage = self.usage.clone().unwrap_or_default();
        let metadata = self.metadata();
        let ChatCompletionStreamState {
            response_id,
            finish_reason,
            text,
            tool_calls,
            dsml_healer,
            ..
        } = self;
        let content = dsml_healer.heal_response_content(Self::ordered_content(text, tool_calls));

        ModelResponse {
            id: response_id,
            content,
            stop_reason: finish_reason,
            usage,
            metadata,
        }
    }
}

fn tool_call_delta_starts_block(entry: &PartialToolCall) -> bool {
    !entry.id.is_empty() && !entry.name.is_empty()
}

#[derive(Debug, Default)]
struct StreamTextBlock {
    value: String,
    started: bool,
    finished: bool,
}

#[derive(Debug, Clone, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments_json: String,
    started: bool,
    kind: ToolCallKind,
}

impl PartialToolCall {
    fn into_provider_tool_call(self) -> OpenAIChatCompletionMessageToolCall {
        match self.kind {
            ToolCallKind::Function => OpenAIChatCompletionMessageToolCall {
                id: self.id,
                kind: "function".to_string(),
                function: Some(OpenAIChatCompletionFunctionCall {
                    arguments: self.arguments_json,
                    name: self.name,
                }),
                custom: None,
            },
            ToolCallKind::Custom => OpenAIChatCompletionMessageToolCall {
                id: self.id,
                kind: "custom".to_string(),
                function: None,
                custom: Some(OpenAIChatCompletionCustomToolCall {
                    input: self.arguments_json,
                    name: self.name,
                }),
            },
        }
    }
}

#[derive(Debug, Default)]
struct StreamChoiceMetadata {
    index: Option<u32>,
    finish_reason: Option<String>,
    logprobs: Option<OpenAIChoiceLogprobs>,
}

#[derive(Debug, Clone, Copy, Default)]
enum ToolCallKind {
    #[default]
    Function,
    Custom,
}

impl ToolCallKind {
    fn from_wire(value: &str) -> Option<Self> {
        match value {
            "function" => Some(Self::Function),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    fn empty_input(self) -> Value {
        match self {
            Self::Function => Value::Object(serde_json::Map::new()),
            Self::Custom => Value::String(String::new()),
        }
    }
}
