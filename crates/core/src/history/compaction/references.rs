//! Keep a bounded set of lookup keys for tool evidence removed from the prompt.
use std::collections::HashSet;

use crate::ResponseItem;
use crate::context::ContextualUserFragment;
use crate::context::compaction_summary::CompactionSummary;
use devo_protocol::ContentBlock;

const LABEL: &str = "Recorded tool references: ";
const MAX_REFS: usize = 6;

fn valid_key(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
}

pub(super) fn attach(summary: String, prefix: &[ResponseItem]) -> String {
    // Treat any model-generated reference footer as a suggestion. Only emit keys
    // from completed pairs in this prefix or the previous checkpoint's footer.
    let mut eligible = Vec::new();
    let mut calls = HashSet::new();
    for (index, item) in prefix.iter().enumerate() {
        match item {
            ResponseItem::ToolCall { id, .. } => {
                calls.insert(id.as_str());
            }
            ResponseItem::ToolCallOutput { tool_use_id, .. }
                if calls.remove(tool_use_id.as_str()) =>
            {
                if valid_key(tool_use_id) {
                    eligible.push(tool_use_id.as_str());
                }
            }
            ResponseItem::Message(message) if index == 0 => {
                if let [ContentBlock::Text { text }] = message.content.as_slice()
                    && CompactionSummary::matches_text(text)
                    && let Some(footer) = text
                        .lines()
                        .rev()
                        .map(str::trim)
                        .find(|line| !line.is_empty() && *line != CompactionSummary::END_MARKER)
                        .and_then(|line| line.strip_prefix(LABEL))
                {
                    eligible.extend(
                        footer
                            .split_whitespace()
                            .take(MAX_REFS)
                            .filter_map(|key| key.strip_prefix("call:"))
                            .filter(|id| valid_key(id)),
                    );
                }
            }
            _ => {}
        }
    }
    let mut chosen = Vec::new();
    for &id in eligible
        .iter()
        .rev()
        .filter(|&&id| summary.contains(id))
        .chain(eligible.iter().rev())
    {
        if !chosen.contains(&id) {
            chosen.push(id);
            if chosen.len() == MAX_REFS {
                break;
            }
        }
    }
    let mut text = summary
        .lines()
        .filter(|line| !line.trim_start().starts_with(LABEL))
        .collect::<Vec<_>>()
        .join("\n");
    if !chosen.is_empty() {
        text.push_str("\n\n");
        text.push_str(LABEL);
        text.push_str(
            &chosen
                .into_iter()
                .map(|id| format!("call:{id}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    text
}
