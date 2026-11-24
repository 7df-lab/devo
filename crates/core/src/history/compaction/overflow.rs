//! Identify the current user input that must survive overflow recovery.

use std::ops::Range;

use devo_protocol::{ContentBlock, Role};

use crate::ResponseItem;
use crate::context::AgentsMdDiffFragment;
use crate::context::ContextChangesFragment;
use crate::context::ContextualUserFragment;
use crate::context::compaction_summary::CompactionSummary;
use crate::context::turn_aborted::TurnAborted;

/// A single input can expand to adjacent text and image items. Preserve the
/// entire group, excluding internal messages that also use the user role.
pub(super) fn latest_user_input_range(items: &[ResponseItem]) -> Range<usize> {
    let Some(last) = items.iter().rposition(is_user_input) else {
        return 0..0;
    };
    let group_start = |index: usize| {
        items[..index]
            .iter()
            .rposition(|item| !matches!(item, ResponseItem::Message(msg) if msg.role == Role::User))
            .map_or(0, |index| index + 1)
    };
    let mut first = group_start(last);
    // Tool attachments also become user-role image items after flattening.
    // When an image follows a tool result, keep the latest textual input too.
    if first > 0
        && matches!(items[first - 1], ResponseItem::ToolCallOutput { .. })
        && let Some(text_index) = items[..=last].iter().rposition(|item| {
            is_user_input(item)
                && matches!(item, ResponseItem::Message(msg) if msg.content.iter().any(
                    |block| matches!(block, ContentBlock::Text { .. })
                ))
        })
    {
        first = group_start(text_index);
    }
    first..last + 1
}

pub(super) fn is_user_input(item: &ResponseItem) -> bool {
    matches!(item, ResponseItem::Message(message) if is_user_message(message))
}

pub(crate) fn is_user_message(message: &devo_protocol::Message) -> bool {
    message.role == Role::User
        && !message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        && !message.content.iter().any(|block| {
            let ContentBlock::Text { text } = block else {
                return false;
            };
            let text = text.trim_start();
            if text.starts_with("Note: the previous tool call(s) were repeated ")
                || (text.starts_with("[Tool call ") && text.contains(" was blocked: "))
                || ((text.starts_with("[Host notification: ")
                    || text.starts_with("[Background notification: "))
                    && text.contains(crate::HOST_MESSAGE_AUTHORITY_NOTICE))
            {
                return true;
            }
            [
                CompactionSummary::START_MARKER,
                ContextChangesFragment::START_MARKER,
                AgentsMdDiffFragment::START_MARKER,
                TurnAborted::START_MARKER,
            ]
            .iter()
            .any(|marker| text.starts_with(marker))
        })
}
