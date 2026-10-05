//! Packed [`ItemRecord`] expansion for server tests.
//!
//! **Not the live or fork write path.** Live and fork appends build Native
//! [`ItemEnvelope`] and call `append_canonical_item_at`. This helper remains
//! for server tests that seed rollouts from stored record shapes.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use devo_protocol::native::ids::ItemId;
use devo_protocol::native::item::{
    ApprovalDecision, ApprovalDecisionKind, ApprovalScope, ApprovalTarget, CompactionTrigger,
    ContextUsage, ExecOrigin, ExecutionMode, InternalEntry, Item, ItemEnvelope, ItemState,
    ToolSource, UserInput, UserMessageEntry,
};

use crate::conversation::{ApprovalRequestItem, ItemRecord, TextItem, TurnItem};

use super::super::rollout::{InternalRecord, ROLLOUT_FORMAT_VERSION, RolloutLine};
use super::{ApprovalFold, RecordConversionError, RolloutWriteState};

/// Intermediate result of projecting one packed legacy payload: either a
/// normal item (fresh seq assigned by the caller), an approval-fold item
/// (id/seq/revision already fixed by the fold), or a non-item internal
/// record.
#[derive(Debug)]
enum Projected {
    Item {
        item: Item,
        state: ItemState,
    },
    FoldedItem {
        id: ItemId,
        seq: u64,
        revision: u32,
        item: Item,
        state: ItemState,
    },
    Internal(Box<InternalRecord>),
}

impl RolloutWriteState {
    /// Expands one packed legacy item record into one or more v2 lines,
    /// assigning seqs and folding approvals.
    ///
    /// Migrate / test only — not the live or fork write path.
    pub fn item_lines_from_record(
        &mut self,
        record: &ItemRecord,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<RolloutLine>, RecordConversionError> {
        let session_id = record.session_id;
        let turn_id = record.turn_id;
        let first_item_id = record.id;

        let mut out = Vec::new();
        for (index, payload) in record
            .input_items
            .iter()
            .chain(&record.output_items)
            .enumerate()
        {
            // A legacy record packs N payloads under a single record id; the
            // first payload keeps that id, the rest get fresh opaque ids
            // because persistence is one-record-one-item in v2.
            let item_id = if index == 0 {
                first_item_id
            } else {
                ItemId::new()
            };
            let (id, seq, revision, state, item) =
                match self.project_payload(record, item_id, payload)? {
                    Projected::Item { item, state } => (item_id, self.next_seq(), 1, state, item),
                    Projected::FoldedItem {
                        id,
                        seq,
                        revision,
                        item,
                        state,
                    } => (id, seq, revision, state, item),
                    Projected::Internal(entry) => {
                        // Internal entries consume one sequence position, shared
                        // with the item stream, so their order among items is
                        // exactly recoverable by the inverse projector.
                        out.push(RolloutLine::Internal {
                            v: ROLLOUT_FORMAT_VERSION,
                            timestamp,
                            session_id,
                            turn_id: Some(turn_id),
                            seq: self.next_seq(),
                            entry: *entry,
                        });
                        continue;
                    }
                };
            out.push(RolloutLine::Item {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp,
                item: ItemEnvelope {
                    id,
                    session_id,
                    turn_id,
                    seq,
                    revision,
                    created_at: record.started_at.unwrap_or(record.timestamp),
                    updated_at: record.timestamp,
                    state,
                    item,
                    parent_id: None,
                },
            });
        }
        Ok(out)
    }

    fn project_payload(
        &mut self,
        record: &ItemRecord,
        item_id: ItemId,
        payload: &TurnItem,
    ) -> Result<Projected, RecordConversionError> {
        let projected = match payload {
            TurnItem::UserMessage(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::UserMessage {
                    client_user_message_id: None,
                    content: user_message_content(item),
                    entry: UserMessageEntry::TurnStart,
                },
            },
            TurnItem::SteerInput(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::UserMessage {
                    client_user_message_id: None,
                    content: user_message_content(item),
                    entry: UserMessageEntry::Steer,
                },
            },
            TurnItem::HookPrompt(item) => Projected::Internal(Box::new(InternalRecord::Entry {
                entry: InternalEntry::HookPrompt {
                    text: item.text.clone(),
                },
            })),
            TurnItem::AgentMessage(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::AssistantMessage {
                    text: item.text.clone(),
                },
            },
            TurnItem::Plan(item) => Projected::Item {
                state: ItemState::Completed,
                // Structured `update_plan` JSON expands into one entry per
                // step. Proposed Plan markdown stays a single completed entry.
                item: Item::Plan {
                    call_id: None,
                    entries:
                        devo_protocol::native::plan_parse::plan_entries_from_plan_text_or_single(
                            item.text.clone(),
                        ),
                },
            },
            TurnItem::Reasoning(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::Reasoning {
                    text: item.text.clone(),
                    provider_payload_ref: None,
                },
            },
            TurnItem::ToolCall(call) => Projected::Item {
                state: ItemState::Completed,
                item: Item::ToolCall {
                    call_id: call.tool_call_id.clone(),
                    tool_name: call.tool_name.clone(),
                    // Legacy persisted calls all went through the builtin
                    // dispatcher.
                    source: ToolSource::Builtin,
                    server_name: None,
                    input: Some(call.input.clone()),
                },
            },
            TurnItem::ToolProgress(progress) => {
                Projected::Internal(Box::new(InternalRecord::Entry {
                    entry: InternalEntry::ToolProgress {
                        call_id: progress.tool_call_id.clone(),
                        message: progress.message.clone(),
                    },
                }))
            }
            TurnItem::ToolResult(result) => Projected::Item {
                state: ItemState::Completed,
                item: Item::ToolResult {
                    call_id: result.tool_call_id.clone(),
                    output: result.output.clone(),
                    display_content: result.display_content.clone(),
                    is_error: result.is_error,
                    truncated: false,
                },
            },
            TurnItem::CommandExecution(command) => Projected::Item {
                state: ItemState::Completed,
                item: Item::CommandExecution {
                    call_id: command.tool_call_id.clone(),
                    command: command.command.clone(),
                    argv: None,
                    // Legacy exec payloads never recorded a cwd; fall back to
                    // the session cwd, or an explicitly empty path when the
                    // SessionMeta line has not been seen yet.
                    cwd: self.session_cwd.clone().unwrap_or_default(),
                    input: Some(command.input.clone()),
                    output: Some(command.output.clone()),
                    exit_code: None,
                    execution_handle: None,
                    is_error: command.is_error,
                    execution_mode: ExecutionMode::Foreground,
                    origin: ExecOrigin::AgentTool,
                    sandbox: None,
                },
            },
            TurnItem::WebSearch(item) => Projected::Item {
                state: ItemState::Completed,
                // Legacy hosted-tool payloads only kept their rendered text;
                // no call id was recorded, so the envelope item id stands in
                // as a stable identifier.
                item: Item::HostedToolCall {
                    call_id: item_id.as_str().to_owned(),
                    tool_name: "web_search".into(),
                    input: None,
                    output: Some(serde_json::Value::String(item.text.clone())),
                },
            },
            TurnItem::ImageGeneration(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::HostedToolCall {
                    call_id: item_id.as_str().to_owned(),
                    tool_name: "image_generation".into(),
                    input: None,
                    output: Some(serde_json::Value::String(item.text.clone())),
                },
            },
            TurnItem::ContextCompaction(item) => Projected::Item {
                state: ItemState::Completed,
                item: Item::ContextCompaction {
                    // Legacy did not record the trigger; the conservative
                    // default is the automatic threshold.
                    trigger: CompactionTrigger::AutoThreshold,
                    before: ContextUsage {
                        measured: false,
                        ..ContextUsage::default()
                    },
                    after: None,
                    summary: Some(item.text.clone()),
                },
            },
            TurnItem::TurnSummary(item) => Projected::Internal(Box::new(InternalRecord::Entry {
                entry: InternalEntry::TurnSummary {
                    text: item.text.clone(),
                },
            })),
            TurnItem::ApprovalRequest(request) => {
                let seq = self.next_seq();
                self.approvals.insert(
                    request.approval_id.clone(),
                    ApprovalFold {
                        item_id,
                        seq,
                        revision: 1,
                        request: request.clone(),
                    },
                );
                Projected::FoldedItem {
                    id: item_id,
                    seq,
                    revision: 1,
                    state: ItemState::Waiting,
                    item: approval_request_item(request, None),
                }
            }
            TurnItem::ApprovalDecision(decision) => {
                match self.approvals.get_mut(&decision.approval_id) {
                    Some(fold) => {
                        fold.revision += 1;
                        // Legacy decisions were free-form strings; "allow"
                        // appears in historical files (records.rs tests) and
                        // anything not clearly approve/deny is cancelled.
                        let decision_kind = match decision.decision.to_ascii_lowercase().as_str() {
                            "approve" | "approved" | "allow" => ApprovalDecisionKind::Approved,
                            "deny" | "denied" => ApprovalDecisionKind::Denied,
                            _ => ApprovalDecisionKind::Cancelled,
                        };
                        // Unknown legacy scope strings fall back to the
                        // narrowest scope instead of failing the conversion.
                        let scope = match decision.scope.to_ascii_lowercase().as_str() {
                            "once" => ApprovalScope::Once,
                            "turn" => ApprovalScope::Turn,
                            "session" => ApprovalScope::Session,
                            "path_prefix" => ApprovalScope::PathPrefix,
                            "host" => ApprovalScope::Host,
                            "tool" => ApprovalScope::Tool,
                            "command_prefix" => ApprovalScope::CommandPrefix,
                            "command_prefix_persist" => ApprovalScope::CommandPrefixPersist,
                            _ => ApprovalScope::Once,
                        };
                        let request = fold.request.clone();
                        Projected::FoldedItem {
                            id: fold.item_id,
                            seq: fold.seq,
                            revision: fold.revision,
                            state: ItemState::Completed,
                            item: approval_request_item(
                                &request,
                                Some(ApprovalDecision {
                                    decision: decision_kind,
                                    scope,
                                    decision_source: decision.decision_source.unwrap_or_default(),
                                    decided_at: record.timestamp,
                                }),
                            ),
                        }
                    }
                    None => {
                        // Orphan decision (no matching request in this file):
                        // keep the information as a warning item with a fresh
                        // opaque id/seq rather than dropping history.
                        Projected::FoldedItem {
                            id: ItemId::new(),
                            seq: self.next_seq(),
                            revision: 1,
                            state: ItemState::Completed,
                            item: Item::Warning {
                                code: "legacyOrphanApprovalDecision".into(),
                                message: format!(
                                    "approval decision '{}'/'{}' references unknown approval id {}",
                                    decision.decision, decision.scope, decision.approval_id
                                ),
                                retryable: false,
                            },
                        }
                    }
                }
            }
        };
        Ok(projected)
    }
}

/// Builds the approval target from the legacy request's optional path, host,
/// or free-form target string, in that priority order.
fn approval_target(request: &ApprovalRequestItem) -> Option<ApprovalTarget> {
    if let Some(path) = &request.path {
        Some(ApprovalTarget::Path {
            path: PathBuf::from(path),
        })
    } else if let Some(host) = &request.host {
        Some(ApprovalTarget::Host { host: host.clone() })
    } else {
        request
            .target
            .clone()
            .map(|command| ApprovalTarget::Command { command })
    }
}

/// Reconstructs a full `Item::Approval` from a stored legacy request payload,
/// with or without the folded-in decision.
fn approval_request_item(
    request: &ApprovalRequestItem,
    decision: Option<ApprovalDecision>,
) -> Item {
    Item::Approval {
        approval_id: request.approval_id.clone(),
        target_item_id: None,
        action_summary: request.action_summary.clone(),
        justification: request.justification.clone(),
        resource: request.resource.clone(),
        available_scopes: request.available_scopes.clone(),
        command_pattern: request.command_pattern.clone(),
        command_prefix: request.command_prefix.clone(),
        target: approval_target(request),
        decision,
    }
}

/// Builds UserMessage content: text plus LocalImage parts for persisted paths.
fn user_message_content(item: &TextItem) -> Vec<UserInput> {
    let mut content = Vec::with_capacity(1 + item.local_image_paths.len());
    content.push(UserInput::Text {
        text: item.text.clone(),
    });
    for path in &item.local_image_paths {
        content.push(UserInput::LocalImage {
            path: path.clone(),
            detail: None,
        });
    }
    content
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: multimodal LocalImage persistence
    /// Verifies: UserMessage/SteerInput projection includes LocalImage parts
    /// from TextItem.local_image_paths.
    #[test]
    fn user_message_content_includes_local_image_paths() {
        let path = PathBuf::from("/tmp/photo.png");
        let item = TextItem {
            text: "see this".into(),
            local_image_paths: vec![path.clone()],
        };
        assert_eq!(
            user_message_content(&item),
            vec![
                UserInput::Text {
                    text: "see this".into(),
                },
                UserInput::LocalImage { path, detail: None },
            ]
        );
    }
}
