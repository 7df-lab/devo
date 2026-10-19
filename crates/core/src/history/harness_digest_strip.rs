//! Exclude continual-harness digests from compaction summarizer input.
//!
//! Markers must stay aligned with `devo_harness::{HARNESS_DIGEST_PREFIX,
//! HARNESS_DIGEST_HEADING}`. Digests are reattached on the new head via
//! harness helpers after compaction — not summarized into the checkpoint.

use devo_protocol::{ContentBlock, Message};

use crate::response_item::ResponseItem;

const HARNESS_DIGEST_MARKER: &str = "[harness-digest]";
const HARNESS_STATE_TAG: &str = "<harness_state>";
const HARNESS_DIGEST_HEADING: &str = "## Continual harness";

fn text_is_harness_digest(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains(HARNESS_DIGEST_MARKER) || trimmed.contains(HARNESS_STATE_TAG) {
        return strip_harness_digest_spans(trimmed).is_empty();
    }
    if trimmed.starts_with(HARNESS_DIGEST_HEADING) {
        return true;
    }
    false
}

fn strip_harness_digest_spans(text: &str) -> String {
    let mut remaining = text.to_string();
    while let Some(start) = remaining.find(HARNESS_DIGEST_MARKER) {
        let after = &remaining[start..];
        let end = after
            .find("</harness_state>")
            .map(|rel| start + rel + "</harness_state>".len())
            .unwrap_or(remaining.len());
        remaining.replace_range(start..end, "");
    }
    if let Some(start) = remaining.find(HARNESS_DIGEST_HEADING) {
        let rest = &remaining[start..];
        let end = rest
            .find("\n## ")
            .or_else(|| rest.find("\n# "))
            .map(|rel| start + rel)
            .unwrap_or(remaining.len());
        remaining.replace_range(start..end, "");
    }
    remaining.trim().to_string()
}

fn map_message_without_digest(msg: &Message) -> Option<Message> {
    let content: Vec<ContentBlock> = msg
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } if text_is_harness_digest(text) => None,
            ContentBlock::Text { text }
                if text.contains(HARNESS_DIGEST_MARKER)
                    || text.contains(HARNESS_STATE_TAG)
                    || text.contains(HARNESS_DIGEST_HEADING) =>
            {
                let stripped = strip_harness_digest_spans(text);
                if stripped.is_empty() {
                    None
                } else {
                    Some(ContentBlock::Text { text: stripped })
                }
            }
            other => Some(other.clone()),
        })
        .collect();
    if content.is_empty() {
        None
    } else {
        Some(Message {
            role: msg.role,
            content,
        })
    }
}

/// Drop / strip harness digest messages so the summarizer never sees \(H\).
pub(crate) fn exclude_harness_digests(items: &[ResponseItem]) -> Vec<ResponseItem> {
    items
        .iter()
        .filter_map(|item| match item {
            ResponseItem::Message(msg) => {
                map_message_without_digest(msg).map(ResponseItem::Message)
            }
            other => Some(other.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-CONTEXT-002, L2-DES-HARNESS-001
    /// Verifies: summarizer input drops harness digest messages.
    #[test]
    fn excludes_digest_only_messages() {
        let digest = format!(
            "{HARNESS_DIGEST_MARKER}\n\nmemories\n{HARNESS_STATE_TAG}\nkeep\n</harness_state>"
        );
        let items = vec![
            ResponseItem::Message(Message::user(digest)),
            ResponseItem::Message(Message::user("real work")),
            ResponseItem::Message(Message::assistant_text("ok")),
        ];
        let filtered = exclude_harness_digests(&items);
        assert_eq!(filtered.len(), 2);
        match &filtered[0] {
            ResponseItem::Message(msg) => {
                assert!(
                    matches!(&msg.content[0], ContentBlock::Text { text } if text == "real work")
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
