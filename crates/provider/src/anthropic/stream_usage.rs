use devo_protocol::Usage;
use serde_json::Value;

#[derive(Debug, Default)]
pub(super) struct AnthropicStreamUsage {
    reported: bool,
    uncached_input_tokens: Option<usize>,
    output_tokens: Option<usize>,
    reasoning_output_tokens: Option<usize>,
    cache_creation_input_tokens: Option<usize>,
    cache_read_input_tokens: Option<usize>,
}

impl AnthropicStreamUsage {
    pub(super) fn update_from_message_start(&mut self, data: &Value) -> Option<Usage> {
        let usage = data
            .get("message")
            .and_then(|message| message.get("usage"))
            .or_else(|| data.get("usage"))?;
        self.update_from_usage(usage)
    }

    pub(super) fn update_from_message_delta(&mut self, data: &Value) -> Option<Usage> {
        self.update_from_usage(data.get("usage")?)
    }

    pub(super) fn snapshot(&self) -> Usage {
        let cache_creation_input_tokens = self.cache_creation_input_tokens.unwrap_or(0);
        let cache_read_input_tokens = self.cache_read_input_tokens.unwrap_or(0);
        Usage {
            terminal_sample: false,
            unreported_fields: None,
            reported: self.reported,
            input_tokens: self
                .uncached_input_tokens
                .unwrap_or(0)
                .saturating_add(cache_creation_input_tokens)
                .saturating_add(cache_read_input_tokens),
            output_tokens: self.output_tokens.unwrap_or(0),
            cache_creation_input_tokens: self.cache_creation_input_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens,
            total_tokens: None,
        }
        .with_field_coverage(self.uncached_input_tokens, self.output_tokens)
    }

    fn update_from_usage(&mut self, usage: &Value) -> Option<Usage> {
        let mut updated = false;
        if let Some(input_tokens) = usage.get("input_tokens").and_then(Value::as_u64) {
            self.uncached_input_tokens = Some(input_tokens as usize);
            updated = true;
        }
        if let Some(output_tokens) = usage.get("output_tokens").and_then(Value::as_u64) {
            self.output_tokens = Some(output_tokens as usize);
            updated = true;
        }
        if let Some(tokens) = usage
            .pointer("/output_tokens_details/thinking_tokens")
            .and_then(Value::as_u64)
        {
            self.reasoning_output_tokens = Some(tokens as usize);
            updated = true;
        }
        if let Some(cache_creation_input_tokens) = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
        {
            self.cache_creation_input_tokens = Some(cache_creation_input_tokens as usize);
            updated = true;
        }
        if let Some(cache_read_input_tokens) =
            usage.get("cache_read_input_tokens").and_then(Value::as_u64)
        {
            self.cache_read_input_tokens = Some(cache_read_input_tokens as usize);
            updated = true;
        }

        self.reported |= updated;
        updated.then(|| self.snapshot())
    }
}
