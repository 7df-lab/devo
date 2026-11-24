//! Use the last accepted main-model request as the source of a summary prefix.

use std::borrow::Cow;

use devo_protocol::{ModelRequest, RequestMessage, RequestRole};
use tokio_util::sync::CancellationToken;

use super::{
    CompactionConfig, CompactionError, CompactionKind, recent_tail_start,
    summarizer_request_messages,
};
use crate::ResponseItem;
use crate::history::summarizer::DefaultHistorySummarizer;
use crate::query::estimate_request_context_breakdown;

pub(crate) struct AcceptedPrefix {
    // The envelope includes the actual system, tools and locked input messages.
    envelope: ModelRequest,
    history: Vec<RequestMessage>,
}

impl AcceptedPrefix {
    pub(crate) fn capture(request: &ModelRequest, locked: &[RequestMessage]) -> Option<Self> {
        // Hosted tools run on the provider. Use the isolated, tool-free
        // summarizer for those routes and for unknown request-body overrides
        // (which can replace the entire messages array or force a tool call).
        if !request.hosted_tools.is_empty() || !request.messages.starts_with(locked) {
            return None;
        }
        if let Some(extra) = &request.extra_body {
            let fields = extra.as_object()?;
            if !fields.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "__devo_request_headers"
                        | "chat_template_kwargs"
                        | "cache_salt"
                        | "prompt_cache_key"
                        | "prompt_cache_retention"
                        | "thinking"
                        | "reasoning"
                        | "reasoning_effort"
                        | "enable_thinking"
                        | "temperature"
                        | "top_p"
                        | "top_k"
                        | "min_p"
                        | "seed"
                        | "presence_penalty"
                        | "frequency_penalty"
                        | "repetition_penalty"
                        | "max_tokens"
                        | "max_completion_tokens"
                        | "max_output_tokens"
                )
            }) {
                return None;
            }
        }
        let mut envelope = request.clone();
        let history = envelope.messages.split_off(locked.len());
        Some(Self { envelope, history })
    }

    fn request(&self, selected: &[RequestMessage], max_tokens: usize) -> Option<ModelRequest> {
        let (instruction, evidence) = selected.split_last()?;
        if evidence.is_empty() || !self.history.starts_with(evidence) {
            return None;
        }
        let mut request = self.envelope.clone();
        request
            .messages
            .extend_from_slice(&self.history[..evidence.len()]);
        // Some adapters move developer messages into the system prefix.
        // Appending a user instruction leaves the cached system intact.
        request.messages.push(RequestMessage {
            role: RequestRole::User.as_str().into(),
            content: instruction.content.clone(),
        });
        request.max_tokens = max_tokens;
        if let Some(extra) = request.extra_body.as_mut() {
            for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
                if let Some(limit) = extra.get_mut(key) {
                    *limit = limit.as_u64()?.min(max_tokens as u64).into();
                }
            }
        }
        Some(request)
    }
}

pub(crate) struct PrefixAttempt<'a> {
    pub(crate) source: &'a AcceptedPrefix,
    pub(crate) summarizer: &'a DefaultHistorySummarizer,
    pub(crate) max_tokens: usize,
    pub(crate) config: &'a CompactionConfig,
}

impl PrefixAttempt<'_> {
    pub(super) async fn summarize(
        &self,
        items: &[ResponseItem],
        boundaries: &[usize],
        selected_end: usize,
        first_replaced: usize,
        selected_messages: &[RequestMessage],
        cancel: Option<&CancellationToken>,
    ) -> Result<Option<(usize, String)>, CompactionError> {
        let mut end = selected_end;
        let mut messages = Cow::Borrowed(selected_messages);
        let mut minimum_cut = None;
        loop {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(CompactionError::Canceled);
            }
            if let Some(request) = self.source.request(&messages, self.max_tokens)
                && estimate_request_context_breakdown(&request)
                    .total()
                    .saturating_add(self.max_tokens as u64)
                    <= self.config.budget.context_window as u64
            {
                match self.summarizer.complete(request, cancel).await {
                    Ok(summary) => return Ok(Some((end, summary))),
                    Err(CompactionError::Canceled) => return Err(CompactionError::Canceled),
                    Err(CompactionError::ContextTooLong) => {}
                    // A backend may reject tools on its completion endpoint,
                    // or return a tool call. Retry in the isolated summarizer.
                    Err(_) => break,
                }
            }
            // In emergency recovery the whole selected range must be folded.
            // Ordinary rolling compaction can leave more completed work raw.
            if self.config.kind == CompactionKind::OverflowRecovery {
                break;
            }
            // Never retreat past all removable work, or leave a tail that
            // already exceeds the next main request's entire input budget.
            let minimum = *minimum_cut.get_or_insert_with(|| {
                (first_replaced + 1)
                    .max(recent_tail_start(items, self.config.budget.input_budget()))
            });
            let earlier = &boundaries[..boundaries.partition_point(|&cut| cut < end)];
            let usable = &earlier[earlier.partition_point(|&cut| cut < minimum)..];
            let Some(&next) = usable.get(usable.len() / 2) else {
                break;
            };
            end = next;
            messages = Cow::Owned(summarizer_request_messages(&items[..end]));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn preserves_cache_parameters_caps_output_and_requires_exact_history() {
        let locked = Message::user("locked").to_request_message();
        let observation = Message::assistant_text("old evidence").to_request_message();
        let instruction = Message::user("summarize").to_request_message();
        let mut main = ModelRequest {
            model_slug: Default::default(),
            model: "local".into(),
            system: Some("system".into()),
            messages: vec![locked.clone(), observation.clone()],
            max_tokens: 32_768,
            tools: None,
            hosted_tools: vec![],
            sampling: Default::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: Some(json!({"max_completion_tokens": 32768, "cache_salt": "same-slot"})),
        };
        let prefix = AcceptedPrefix::capture(&main, std::slice::from_ref(&locked)).unwrap();
        let summary = prefix
            .request(&[observation, instruction.clone()], 4096)
            .unwrap();
        let mut expected = main.clone();
        expected.messages.push(instruction.clone());
        expected.max_tokens = 4096;
        expected.extra_body =
            Some(json!({"max_completion_tokens": 4096, "cache_salt": "same-slot"}));
        assert_eq!(json!(summary), json!(expected));
        assert!(
            prefix
                .request(&[locked.clone(), instruction], 4096)
                .is_none()
        );
        main.extra_body = Some(json!({"messages": []}));
        assert!(AcceptedPrefix::capture(&main, &[locked]).is_none());
    }
}
