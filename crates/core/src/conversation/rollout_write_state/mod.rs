//! Write-path sequence, approval, and settings-epoch fold state for rollout
//! appends.
//!
//! Typed wrappers build [`RolloutLine`] via [`super::rollout_write`] (or
//! Native builders), then append and [`RolloutWriteState::observe_rollout_line`]
//! updates this state. Packed [`crate::conversation::ItemRecord`] expansion remains available to the
//! test-only `RolloutStore::append_item` helper.

pub(crate) mod packed_expand;

use std::collections::HashMap;
use std::path::PathBuf;

use devo_protocol::native::ids::ItemId;
use devo_protocol::native::item::{ApprovalTarget, Item};

use crate::conversation::ApprovalRequestItem;

use super::rollout::{InternalRecord, RolloutLine};

/// Errors from converting stored record shapes into canonical rollout
/// records. Unrecoverable data fails loudly instead of being fabricated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordConversionError {
    /// A legacy identifier did not parse back into the UUID it wraps.
    #[error("legacy identifier is not a valid UUID: {0}")]
    InvalidLegacyId(String),
}

/// The state carried between the request and the decision of one approval.
#[derive(Debug)]
struct ApprovalFold {
    item_id: ItemId,
    seq: u64,
    revision: u32,
    /// The full original request payload, needed to reconstruct the complete
    /// `Item::Approval` when the matching decision arrives.
    request: ApprovalRequestItem,
}

/// Per-session write-path state: item `seq`, settings epoch, approval folds,
/// and the session cwd fallback for legacy CommandExecution payloads.
///
/// Not a live projector — it observes canonical rollout lines to maintain
/// append state.
#[derive(Debug)]
pub struct RolloutWriteState {
    /// Next sequence number to assign on an item's first appearance. Starts
    /// at 1 and is strictly increasing within the session.
    next_seq: u64,
    /// Next epoch to assign to a field-level session settings line
    /// (L2-DES-CONV-002 DD-4). Hydrated from existing v2 lines and strictly
    /// increasing within the session.
    next_settings_epoch: u64,
    /// Session cwd learned from SessionMeta; the fallback `CommandExecution`
    /// cwd because legacy exec payloads never recorded one.
    session_cwd: Option<PathBuf>,
    /// Approval requests seen so far, keyed by `approval_id`, so a later
    /// decision folds into the same item id/seq with a bumped revision.
    approvals: HashMap<String, ApprovalFold>,
}

impl Default for RolloutWriteState {
    fn default() -> Self {
        Self::new()
    }
}

impl RolloutWriteState {
    pub fn new() -> Self {
        Self {
            next_seq: 1,
            next_settings_epoch: 1,
            session_cwd: None,
            approvals: HashMap::new(),
        }
    }

    /// The epoch the next settings write will receive; `1` when no settings
    /// line has been written yet.
    pub fn next_settings_epoch(&self) -> u64 {
        self.next_settings_epoch
    }

    /// Allocates and advances the next settings epoch for a live settings
    /// append (caller passes a placeholder epoch today).
    pub fn allocate_settings_epoch(&mut self) -> u64 {
        let epoch = self.next_settings_epoch;
        self.next_settings_epoch = self.next_settings_epoch.saturating_add(1);
        epoch
    }

    /// Records session cwd for CommandExecution fallback (migrate path when
    /// projecting a SessionMeta line before items).
    pub fn note_session_cwd(&mut self, cwd: PathBuf) {
        self.session_cwd = Some(cwd);
    }

    fn next_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// Re-syncs the write-path state (seq counter, approval folds, cwd) with
    /// a v2 line that is already on disk or about to be written. Stores
    /// hydrating write state for a pre-existing file feed every on-disk v2
    /// line through this method so subsequent appends never collide with or
    /// orphan the on-disk history.
    pub fn observe_rollout_line(&mut self, line: &RolloutLine) {
        match line {
            RolloutLine::SessionMeta { session, .. } => {
                self.session_cwd = Some(session.cwd.clone());
            }
            RolloutLine::Item { item, .. } => {
                self.next_seq = self.next_seq.max(item.seq + 1);
                if let Item::Approval {
                    approval_id,
                    action_summary,
                    justification,
                    resource,
                    available_scopes,
                    command_pattern,
                    command_prefix,
                    target,
                    decision,
                    ..
                } = &item.item
                {
                    match decision {
                        None => {
                            self.approvals.insert(
                                approval_id.clone(),
                                ApprovalFold {
                                    item_id: item.id,
                                    seq: item.seq,
                                    revision: item.revision,
                                    request: approval_request_from_parts(
                                        approval_id,
                                        action_summary,
                                        justification,
                                        resource,
                                        available_scopes,
                                        command_pattern,
                                        command_prefix,
                                        target,
                                    ),
                                },
                            );
                        }
                        Some(_) => {
                            if let Some(fold) = self.approvals.get_mut(approval_id) {
                                fold.revision = fold.revision.max(item.revision);
                            }
                        }
                    }
                }
            }
            RolloutLine::Internal { seq, entry, .. } => {
                self.next_seq = self.next_seq.max(seq + 1);
                if let InternalRecord::SessionSettings { epoch, .. } = entry {
                    self.next_settings_epoch = self.next_settings_epoch.max(epoch + 1);
                }
            }
            RolloutLine::Turn { .. }
            | RolloutLine::SessionTitleUpdated { .. }
            | RolloutLine::CompactionSnapshot { .. }
            | RolloutLine::SessionRollback { .. }
            | RolloutLine::WorkspaceCheckpoint { .. }
            | RolloutLine::WorkspaceChange { .. }
            | RolloutLine::WorkspaceRestoreStarted { .. }
            | RolloutLine::WorkspaceRestoreCompleted { .. } => {}
        }
    }
}

/// Rebuilds a legacy approval request payload from the canonical approval
/// parts; used when hydrating the fold map from an on-disk v2 approval
/// envelope.
#[allow(clippy::too_many_arguments)]
fn approval_request_from_parts(
    approval_id: &str,
    action_summary: &str,
    justification: &str,
    resource: &Option<String>,
    available_scopes: &[String],
    command_pattern: &Option<Vec<String>>,
    command_prefix: &Option<Vec<String>>,
    target: &Option<ApprovalTarget>,
) -> ApprovalRequestItem {
    let (path, host, target) = target
        .as_ref()
        .map_or((None, None, None), |target| match target {
            ApprovalTarget::Path { path } => (Some(path.display().to_string()), None, None),
            ApprovalTarget::Host { host } => (None, Some(host.clone()), None),
            ApprovalTarget::Command { command } => (None, None, Some(command.clone())),
        });
    ApprovalRequestItem {
        approval_id: approval_id.into(),
        action_summary: action_summary.into(),
        justification: justification.into(),
        resource: resource.clone(),
        available_scopes: available_scopes.into(),
        command_pattern: command_pattern.clone(),
        command_prefix: command_prefix.clone(),
        path,
        host,
        target,
    }
}
