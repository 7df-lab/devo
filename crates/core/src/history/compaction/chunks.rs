//! Build one checkpoint even when the selected history exceeds the summarizer's window.
//! Partial summaries stay local until every selected fragment has been consumed.

use std::time::Duration;

use devo_protocol::{RequestContent, RequestMessage, RequestRole};
use tokio_util::sync::CancellationToken;

use super::{CompactionError, HistorySummarizer, sleep_cancellable};

const MAX_FRAGMENT_CALLS: usize = 64;

pub(super) async fn summarize(
    summarizer: &dyn HistorySummarizer,
    mut messages: Vec<RequestMessage>,
    cancel: Option<&CancellationToken>,
) -> Result<String, CompactionError> {
    match summarize_part(summarizer, &messages, cancel).await {
        Err(CompactionError::ContextTooLong) => {}
        result => return result,
    }

    let instruction = messages
        .pop()
        .expect("summary requests end with an instruction");
    let mut evidence = Vec::new();
    for message in messages {
        for block in message.content {
            // Images remain native inputs. Textual tool traffic is historical
            // evidence in these private requests and cannot execute a tool.
            // Signed or encrypted replay data has no textual evidence to summarize.
            if matches!(block, RequestContent::ProviderReasoning { .. }) {
                continue;
            }
            if matches!(block, RequestContent::Image { .. }) {
                evidence.push(block);
            } else {
                let record = serde_json::json!({"role": message.role, "content": block});
                evidence.push(RequestContent::Text {
                    text: record.to_string(),
                });
            }
        }
    }
    let mut pending = Vec::new();
    split_fragment(evidence, &mut pending)?;
    let mut checkpoint = String::new();
    for _ in 0..MAX_FRAGMENT_CALLS {
        let Some(fragment) = pending.pop() else { break };
        let mut content = vec![RequestContent::Text {
            text: format!(
                "Continue the checkpoint using the next chronological fragment of historical records. Retain prior decisions, tool effects, references and unfinished work.\nCheckpoint so far:\n{checkpoint}\nNext fragment:\n"
            ),
        }];
        content.extend_from_slice(&fragment);
        let part = [
            RequestMessage {
                role: RequestRole::User.as_str().into(),
                content,
            },
            instruction.clone(),
        ];
        match summarize_part(summarizer, &part, cancel).await {
            Ok(updated) => checkpoint = updated,
            Err(CompactionError::ContextTooLong) => split_fragment(fragment, &mut pending)?,
            Err(error) => return Err(error),
        }
    }
    if pending.is_empty() {
        Ok(checkpoint)
    } else {
        Err(CompactionError::NotPossible {
            retries: MAX_FRAGMENT_CALLS as u32,
        })
    }
}

fn split_fragment(
    mut fragment: Vec<RequestContent>,
    pending: &mut Vec<Vec<RequestContent>>,
) -> Result<(), CompactionError> {
    let right = if fragment.len() > 1 {
        fragment.split_off(fragment.len() / 2)
    } else if let [RequestContent::Text { text }] = fragment.as_mut_slice() {
        let mut mid = text.len() / 2;
        while !text.is_char_boundary(mid) {
            mid += 1;
        }
        if mid == text.len() {
            mid = text
                .char_indices()
                .next_back()
                .map_or(0, |(offset, _)| offset);
        }
        if mid == 0 {
            return Err(CompactionError::ContextTooLong);
        }
        vec![RequestContent::Text {
            text: text.split_off(mid),
        }]
    } else {
        return Err(CompactionError::ContextTooLong);
    };
    pending.push(right);
    pending.push(fragment);
    Ok(())
}

async fn summarize_part(
    summarizer: &dyn HistorySummarizer,
    messages: &[RequestMessage],
    cancel: Option<&CancellationToken>,
) -> Result<String, CompactionError> {
    let mut attempt = 1;
    loop {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(CompactionError::Canceled);
        }
        match summarizer.summarize(messages.to_vec(), cancel).await {
            Ok(summary) if summary.trim().is_empty() => return Err(CompactionError::EmptyResponse),
            Err(CompactionError::SummarizationFailed { .. }) if attempt < 5 => {
                sleep_cancellable(Duration::from_millis(100 * (1 << attempt)), cancel).await?;
                attempt += 1;
            }
            result => return result,
        }
    }
}
