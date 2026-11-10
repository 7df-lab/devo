//! Canonical history reader: loads a session's effective history from its
//! versioned rollout file in canonical form.
//!
//! Used by the paged history read API (`session/turns/list`,
//! `session/items/list`). The in-memory runtime model deliberately does not
//! retain turn records or item envelopes, so the rollout is the only complete
//! source. A read re-parses the
//! whole file; history reads are infrequent enough that this beats keeping
//! a second in-memory copy in sync (a cache can be added later behind the
//! same function).

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read};
use std::path::Path;

use chrono::{DateTime, Utc};
use devo_protocol::native::ids::{ItemId, SessionId};
use devo_protocol::native::item::ItemEnvelope;
use devo_protocol::native::session::Session;
use devo_protocol::native::turn::Turn;

use super::rollout::{InternalRecord, RolloutLine, RolloutLineReadError, parse_rollout_line};

/// A session's effective canonical history, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CanonicalHistory {
    /// The session metadata line, when the file has one (files always do for
    /// durable sessions; `None` only for truncated reads).
    pub session: Option<Box<Session>>,
    /// Turn records in ascending `sequence` order.
    pub turns: Vec<Turn>,
    /// Item envelopes in ascending `seq` order, approval folds applied.
    pub items: Vec<ItemEnvelope>,
    /// Latest approval-resume checkpoints keyed by approval id (last wins).
    pub approval_checkpoints: std::collections::HashMap<
        String,
        crate::durable_record::TurnApprovalCheckpointRecordedRecord,
    >,
    /// Latest context-window occupancy observed while reading the rollout
    /// (turn extras or compaction snapshots), when present.
    pub latest_context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
    /// Durable parent pointers for the in-session transcript tree (last edge wins).
    pub tree_edges: HashMap<ItemId, Option<ItemId>>,
    /// Current transcript-tree tip (last `SessionLeaf` wins).
    pub leaf_id: Option<ItemId>,
    /// Write sequence for the current leaf; used to ignore stale leaf lines.
    pub leaf_epoch: u64,
}

/// Errors from reading a rollout file as canonical history.
#[derive(Debug, thiserror::Error)]
pub enum HistoryReadError {
    /// The file could not be read.
    #[error("read rollout history: {0}")]
    Io(#[from] std::io::Error),
    /// A line failed the version dispatch. History reads are fail-closed,
    /// like resume: a damaged file errors rather than silently truncating
    /// the returned history.
    #[error("rollout history line {line_index} is unreadable: {error}")]
    DamagedLine {
        line_index: usize,
        error: RolloutLineReadError,
    },
    /// The requested logical session has a durable hard-delete marker in the
    /// journal. Shared journals retain the file for the root and siblings, so
    /// absence of data is not enough to distinguish "deleted" from "not yet
    /// written"; callers map this to a not-found/resume error.
    #[error("session {session_id} was deleted at {deleted_at}")]
    SessionDeleted {
        session_id: SessionId,
        deleted_at: DateTime<Utc>,
    },
}

fn visit_rollout_file_lines(
    path: &Path,
    visit: impl FnMut(usize, &str, bool) -> Result<(), HistoryReadError>,
) -> Result<(), HistoryReadError> {
    let file = std::fs::File::open(path)?;
    // A rollout is append-only. Bound this read to the length observed at
    // open so a concurrent append cannot turn an in-flight truncated tail
    // into a seemingly damaged middle line.
    let snapshot_len = file.metadata()?.len();
    visit_rollout_lines(file.take(snapshot_len), visit)
}

fn visit_rollout_lines(
    reader: impl std::io::Read,
    mut visit: impl FnMut(usize, &str, bool) -> Result<(), HistoryReadError>,
) -> Result<(), HistoryReadError> {
    let reader = std::io::BufReader::new(reader);
    let mut lines = reader.lines().enumerate().peekable();
    while let Some((index, raw)) = lines.next() {
        let raw = raw?;
        let is_last = match lines.peek() {
            Some((_, Ok(_))) => false,
            Some((_, Err(_))) => {
                let Some((_, Err(error))) = lines.next() else {
                    unreachable!("peeked I/O failure should remain queued");
                };
                return Err(error.into());
            }
            None => true,
        };
        visit(index, &raw, is_last)?;
    }
    Ok(())
}

/// Parses one physical JSONL row into a rollout line. Blank rows and a
/// truncated final row are crash tails: they were never acknowledged, so they
/// are skipped. Any other parse failure is fail-closed.
fn parse_history_rollout_line(
    index: usize,
    raw: &str,
    is_last: bool,
) -> Result<Option<RolloutLine>, HistoryReadError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    match parse_rollout_line(raw) {
        Ok(line) => Ok(Some(line)),
        Err(RolloutLineReadError::TruncatedTail) if is_last => Ok(None),
        Err(error) => Err(HistoryReadError::DamagedLine {
            line_index: index,
            error,
        }),
    }
}

/// Visits only the lines owned by `session_id`. Tombstones are checked before
/// the caller's prefilter so a deletion marker is never skipped just because
/// the caller only wants headers or leaf pointers.
fn visit_rollout_file_lines_for_session(
    path: &Path,
    session_id: &SessionId,
    prefilter: impl Fn(&str) -> bool,
    mut visit: impl FnMut(usize, RolloutLine) -> Result<(), HistoryReadError>,
) -> Result<(), HistoryReadError> {
    visit_rollout_file_lines(path, |index, raw, is_last| {
        let tombstone_candidate = raw.contains("\"sessionDeleted\"");
        if !tombstone_candidate && !prefilter(raw) {
            return Ok(());
        }
        let Some(line) = parse_history_rollout_line(index, raw, is_last)? else {
            return Ok(());
        };
        if let RolloutLine::Internal {
            session_id: owner,
            entry: InternalRecord::SessionDeleted { deleted_at, .. },
            ..
        } = &line
            && owner == session_id
        {
            return Err(HistoryReadError::SessionDeleted {
                session_id: *session_id,
                deleted_at: deleted_at.to_owned(),
            });
        }
        if !prefilter(raw) {
            return Ok(());
        }
        if line.session_id().as_ref() != Some(session_id) {
            return Ok(());
        }
        visit(index, line)
    })
}

/// Reads one versioned rollout file into canonical history form. Every line
/// must use the current format version; no older-file projection is attempted.
/// A truncated final line is tolerated as a crash tail, matching resume.
///
/// Rollback markers are honored at turn granularity: the last
/// `SessionRollback` line drops already-read turns (and their items) that
/// are not in its retained set. Item-level retention ids do not change this
/// behavior because rollback truncates at turn boundaries.
pub fn read_canonical_history(path: &Path) -> Result<CanonicalHistory, HistoryReadError> {
    let mut history = CanonicalHistory::default();
    visit_rollout_file_lines(path, |index, raw, is_last| {
        let Some(line) = parse_history_rollout_line(index, raw, is_last)? else {
            return Ok(());
        };
        apply_rollout_line(&mut history, line);
        Ok(())
    })?;
    Ok(history)
}

/// Reads canonical history for one logical session inside a physical journal.
///
/// A shared root journal contains many sessions; this is the production read
/// for both shared and legacy single-session files. Lines owned by other
/// sessions are skipped, and the target session's hard-delete tombstone is
/// reported as [`HistoryReadError::SessionDeleted`].
pub fn read_canonical_history_for_session(
    path: &Path,
    session_id: &SessionId,
) -> Result<CanonicalHistory, HistoryReadError> {
    let mut history = CanonicalHistory::default();
    visit_rollout_file_lines_for_session(
        path,
        session_id,
        |_| true,
        |_, line| {
            apply_rollout_line(&mut history, line);
            Ok(())
        },
    )?;
    Ok(history)
}

/// Reads only the latest folded session header, skipping the typed parse
/// of every line that cannot affect it. Callers that need items, turns,
/// checkpoints, or the leaf pointer must use [`read_canonical_history`];
/// this exists for hot paths (session snapshots on resume) where the folded
/// header alone decides the response. Only three line kinds can shape the
/// header — the meta line, field-level settings folds, and title updates —
/// so the substring pre-filter mirrors those serde tags. A non-header line
/// that merely contains a tag substring still parses and is discarded by
/// the typed match, so the filter can never fabricate a result — it only
/// avoids work. Fold semantics match the internal `apply_rollout_line` helper.
pub fn read_rollout_session_meta(path: &Path) -> Result<Option<Box<Session>>, HistoryReadError> {
    let mut history = CanonicalHistory::default();
    visit_rollout_file_lines(path, |index, raw, is_last| {
        if !raw.contains("\"sessionMeta\"")
            && !raw.contains("\"sessionSettings\"")
            && !raw.contains("\"sessionTitleUpdated\"")
        {
            return Ok(());
        }
        let Some(line) = parse_history_rollout_line(index, raw, is_last)? else {
            return Ok(());
        };
        apply_rollout_line(&mut history, line);
        Ok(())
    })?;
    Ok(history.session)
}

/// Session-scoped variant of [`read_rollout_session_meta`] for shared
/// journals. A later child `SessionMeta` can never replace the requested
/// root's header.
pub fn read_rollout_session_meta_for_session(
    path: &Path,
    session_id: &SessionId,
) -> Result<Option<Box<Session>>, HistoryReadError> {
    let mut history = CanonicalHistory::default();
    visit_rollout_file_lines_for_session(
        path,
        session_id,
        |raw| {
            raw.contains("\"sessionMeta\"")
                || raw.contains("\"sessionSettings\"")
                || raw.contains("\"sessionTitleUpdated\"")
        },
        |_, line| {
            apply_rollout_line(&mut history, line);
            Ok(())
        },
    )?;
    Ok(history.session)
}

/// Reads only the effective transcript-tree tip, avoiding the full typed
/// history parse on actor construction. Callers that need items, turns, or
/// tree edges must use [`read_canonical_history`]. The `sessionLeaf` tag is
/// only a prefilter: matching lines are decoded and folded by
/// the internal `apply_rollout_line` helper, which preserves the stale-epoch and same-epoch
/// last-write semantics shared with the canonical reader.
pub fn read_rollout_session_leaf(path: &Path) -> Result<(Option<ItemId>, u64), HistoryReadError> {
    let mut history = CanonicalHistory::default();
    visit_rollout_file_lines(path, |index, raw, is_last| {
        if !raw.contains("\"sessionLeaf\"") {
            return Ok(());
        }
        let Some(line) = parse_history_rollout_line(index, raw, is_last)? else {
            return Ok(());
        };
        apply_rollout_line(&mut history, line);
        Ok(())
    })?;
    Ok((history.leaf_id, history.leaf_epoch))
}

/// Session-scoped variant of [`read_rollout_session_leaf`] for shared
/// journals. A child's later leaf line must not replace the root tip.
pub fn read_rollout_session_leaf_for_session(
    path: &Path,
    session_id: &SessionId,
) -> Result<(Option<ItemId>, u64), HistoryReadError> {
    let mut history = CanonicalHistory::default();
    visit_rollout_file_lines_for_session(
        path,
        session_id,
        |raw| raw.contains("\"sessionLeaf\""),
        |_, line| {
            apply_rollout_line(&mut history, line);
            Ok(())
        },
    )?;
    Ok((history.leaf_id, history.leaf_epoch))
}

fn apply_rollout_line(history: &mut CanonicalHistory, line: RolloutLine) {
    match line {
        RolloutLine::SessionMeta { session, .. } => history.session = Some(session),
        RolloutLine::Turn { turn, extras, .. } => {
            // A turn is journaled twice: a running line at start and a
            // terminal line at completion. The canonical projection is one
            // record per turn (terminal state wins, first position kept) —
            // appending both made turns/list render every turn twice.
            if let Some(existing) = history
                .turns
                .iter_mut()
                .find(|existing| existing.id == turn.id)
            {
                *existing = *turn;
            } else {
                history.turns.push(*turn);
            }
            if let Some(extras) = extras
                .as_ref()
                .and_then(|extras| extras.context_occupancy.clone())
            {
                history.latest_context_occupancy = Some(extras);
            }
        }
        RolloutLine::Item { item, .. } => history.items.push(item),
        RolloutLine::Internal {
            entry: InternalRecord::TurnApprovalCheckpoint(checkpoint),
            ..
        } => {
            history
                .approval_checkpoints
                .insert(checkpoint.approval_id.clone(), (*checkpoint).clone());
        }
        RolloutLine::Internal {
            entry:
                InternalRecord::SessionSettings {
                    field,
                    value,
                    epoch,
                    ..
                },
            ..
        } => {
            // Field-level settings lines (L2-DES-CONV-002 DD-4) fold into the
            // canonical session snapshot: the last line per field wins, and
            // the settings epoch raises the session version so canonical
            // readers observe the mutation.
            if let Some(session) = history.session.as_mut() {
                apply_settings_to_canonical_session(session, field, value);
                // `epoch + 1`: the SessionMeta-projected version starts at 1,
                // and the first settings write must already observe a bump.
                session.version = session.version.max(epoch + 1);
            }
        }
        RolloutLine::Internal {
            entry: InternalRecord::SessionLeaf { epoch, leaf_id },
            ..
        } => {
            if epoch >= history.leaf_epoch {
                history.leaf_epoch = epoch;
                history.leaf_id = leaf_id;
            }
        }
        RolloutLine::Internal {
            entry:
                InternalRecord::TreeEdge {
                    child_id,
                    parent_id,
                },
            ..
        } => {
            history.tree_edges.insert(child_id, parent_id);
        }
        RolloutLine::SessionTitleUpdated { title, .. } => {
            // Title changes are session metadata; fold them so canonical
            // readers (including the metadata-update response path) see the
            // latest title.
            if let Some(session) = history.session.as_mut() {
                session.title = Some(title);
            }
        }
        RolloutLine::SessionRollback {
            retained_turn_ids, ..
        } => {
            let retained: HashSet<&str> = retained_turn_ids.iter().map(|id| id.as_str()).collect();
            history
                .turns
                .retain(|turn| retained.contains(turn.id.as_str()));
            history
                .items
                .retain(|item| retained.contains(item.turn_id.as_str()));
        }
        // Other internal entries are not items; compaction snapshots shape
        // the prompt, not the displayed history; workspace lines are not part
        // of the conversational timeline.
        RolloutLine::Internal { .. }
        | RolloutLine::WorkspaceCheckpoint { .. }
        | RolloutLine::WorkspaceChange { .. }
        | RolloutLine::WorkspaceRestoreStarted { .. }
        | RolloutLine::WorkspaceRestoreCompleted { .. } => {}
        RolloutLine::CompactionSnapshot {
            context_occupancy, ..
        } => {
            if let Some(occupancy) = context_occupancy {
                history.latest_context_occupancy = Some(occupancy);
            }
        }
    }
}

/// Folds one field-level settings line into the canonical session snapshot.
/// Model-family fields live on `Session::model` / the session record rather
/// than `SessionSettings`; they are intentionally not folded here.
fn apply_settings_to_canonical_session(
    session: &mut devo_protocol::native::session::Session,
    field: crate::conversation::records::SessionSettingsField,
    value: serde_json::Value,
) {
    use crate::conversation::records::SessionSettingsField;
    match field {
        SessionSettingsField::PermissionPreset => {
            // The persisted value uses the legacy `PermissionPreset` wire
            // shape (kebab-case); map it onto the canonical profile enum.
            if let Ok(preset) = serde_json::from_value::<devo_protocol::PermissionPreset>(value) {
                session.settings.permission_profile = match preset {
                    devo_protocol::PermissionPreset::Default => {
                        devo_protocol::native::model::PermissionProfile::Default
                    }
                    devo_protocol::PermissionPreset::AutoReview => {
                        devo_protocol::native::model::PermissionProfile::AutoReview
                    }
                    devo_protocol::PermissionPreset::FullAccess => {
                        devo_protocol::native::model::PermissionProfile::FullAccess
                    }
                };
            }
        }
        SessionSettingsField::SandboxProfile => {
            if let Ok(name) = serde_json::from_value::<String>(value) {
                session.settings.sandbox_profile = Some(name);
            }
        }
        SessionSettingsField::ReasoningEffortSelection => {
            // The stored value is the user's selection literal, including the
            // toggle keywords (`on`/`off`, plus legacy `enabled`/`disabled`)
            // the `ReasoningEffort` enum cannot express — keep it normalized
            // instead of parsing, which silently dropped those and broke restore.
            if let Ok(Some(raw)) = serde_json::from_value::<Option<String>>(value) {
                let normalized = devo_protocol::normalize_reasoning_effort_literal(&raw);
                if !normalized.is_empty() {
                    session.settings.reasoning_effort = Some(normalized);
                }
            }
        }
        SessionSettingsField::CollaborationMode => {
            if let Ok(raw) = serde_json::from_value::<String>(value) {
                session.settings.mode = Some(raw);
            }
        }
        SessionSettingsField::Model => {
            // Model lives on `Session::model`, not `SessionSettings`; fold the
            // slug so canonical readers observe the switch.
            if let Ok(Some(slug)) = serde_json::from_value::<Option<String>>(value) {
                session.model.model = slug;
            }
        }
        SessionSettingsField::ModelBindingId => {}
        SessionSettingsField::AutoRefineEnabled => {
            if let Ok(enabled) = serde_json::from_value::<bool>(value) {
                session.settings.auto_refine_enabled = Some(enabled);
            }
        }
        SessionSettingsField::AutoRefineTurnInterval => {
            if let Ok(interval) = serde_json::from_value::<u32>(value) {
                session.settings.auto_refine_turn_interval = Some(interval.max(1));
            }
        }
        SessionSettingsField::PythonCellFirstWaitMs => {
            if let Ok(ms) = serde_json::from_value::<u64>(value) {
                session.settings.python_cell_first_wait_ms = Some(ms);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::conversation::records::SessionSettingsField;
    use crate::conversation::rollout::{InternalRecord, ROLLOUT_FORMAT_VERSION, RolloutLine};
    use crate::conversation::rollout_write::native_session_from_record;
    use crate::conversation::{ItemId, SessionId, TurnId};
    use devo_protocol::native::item::{Item, ItemEnvelope, ItemState};

    fn fixture_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap()
    }

    fn write_lines(path: &Path, lines: &[RolloutLine]) {
        let mut text = String::new();
        for line in lines {
            text.push_str(&serde_json::to_string(line).expect("serialize"));
            text.push('\n');
        }
        std::fs::write(path, text).expect("write fixture");
    }

    fn assistant_item_envelope(
        session_id: SessionId,
        turn_id: TurnId,
        seq: u64,
        text: &str,
    ) -> ItemEnvelope {
        ItemEnvelope {
            id: ItemId::new(),
            session_id,
            turn_id,
            seq,
            revision: 1,
            created_at: fixture_now(),
            updated_at: fixture_now(),
            state: ItemState::Completed,
            item: Item::AssistantMessage {
                text: text.to_string(),
            },
            parent_id: None,
        }
    }

    fn settings_line(
        timestamp: chrono::DateTime<Utc>,
        session_id: SessionId,
        field: SessionSettingsField,
        value: serde_json::Value,
        epoch: u64,
    ) -> RolloutLine {
        RolloutLine::Internal {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp,
            session_id,
            turn_id: None,
            seq: 0,
            entry: InternalRecord::SessionSettings {
                schema_version: 1,
                field,
                value,
                epoch,
            },
        }
    }

    fn native_turn(
        status: devo_protocol::native::turn::TurnStatus,
        id: TurnId,
        sequence: u32,
    ) -> devo_protocol::native::turn::Turn {
        devo_protocol::native::turn::Turn {
            id,
            session_id: SessionId::new(),
            sequence,
            kind: devo_protocol::native::turn::TurnKind::Regular,
            status,
            model: devo_protocol::native::model::ModelBinding {
                provider: "test".into(),
                model: "test-model".into(),
                variant: None,
                reasoning_effort: None,
            },
            collaboration_mode: None,
            started_at: Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap(),
            completed_at: None,
            error: None,
            usage: None,
        }
    }

    #[test]
    fn running_and_terminal_turn_lines_collapse_into_one_record() {
        // A turn is journaled as a running line at start and a terminal line
        // at completion; the canonical projection must keep ONE record per
        // turn (terminal state wins, first position kept) — turns/list used
        // to render every turn twice.
        let now = Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap();
        let first = TurnId::new();
        let second = TurnId::new();
        let mut terminal_first =
            native_turn(devo_protocol::native::turn::TurnStatus::Completed, first, 1);
        terminal_first.completed_at = Some(now);
        let mut history = CanonicalHistory::default();
        for line in [
            RolloutLine::Turn {
                v: 2,
                timestamp: now,
                turn: Box::new(native_turn(
                    devo_protocol::native::turn::TurnStatus::InProgress,
                    first,
                    1,
                )),
                extras: None,
            },
            // A later turn's running line must not take the first turn's
            // slot: records keep their first position in file order.
            RolloutLine::Turn {
                v: 2,
                timestamp: now,
                turn: Box::new(native_turn(
                    devo_protocol::native::turn::TurnStatus::InProgress,
                    second,
                    2,
                )),
                extras: None,
            },
            RolloutLine::Turn {
                v: 2,
                timestamp: now,
                turn: Box::new(terminal_first),
                extras: None,
            },
        ] {
            let serialized = serde_json::to_value(&line).unwrap();
            if let RolloutLine::Turn { turn, .. } = &line {
                assert_eq!(
                    serialized["turn"],
                    serde_json::to_value(turn.as_ref()).unwrap()
                );
            }
            assert_eq!(
                serde_json::from_value::<RolloutLine>(serialized).unwrap(),
                line
            );
            apply_rollout_line(&mut history, line);
        }
        assert_eq!(history.turns.len(), 2);
        assert_eq!(history.turns[0].id, first);
        assert_eq!(
            history.turns[0].status,
            devo_protocol::native::turn::TurnStatus::Completed
        );
        assert_eq!(history.turns[0].completed_at, Some(now));
        assert_eq!(history.turns[1].id, second);
        assert_eq!(
            history.turns[1].status,
            devo_protocol::native::turn::TurnStatus::InProgress
        );
    }

    #[test]
    fn rollback_truncates_turns_and_their_items() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let kept_turn = TurnId::new();
        let dropped_turn = TurnId::new();
        let kept_item = assistant_item_envelope(session_id, kept_turn, 1, "kept");
        let dropped_item = assistant_item_envelope(session_id, dropped_turn, 2, "dropped");
        write_lines(
            &dir.path().join("rollout.jsonl"),
            &[
                RolloutLine::Item {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: kept_item.updated_at,
                    item: kept_item,
                },
                RolloutLine::Item {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: dropped_item.updated_at,
                    item: dropped_item,
                },
                RolloutLine::SessionRollback {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: Utc.with_ymd_and_hms(2026, 7, 1, 12, 1, 0).unwrap(),
                    session_id,
                    retained_turn_ids: vec![kept_turn],
                    retained_item_ids: Vec::new(),
                    latest_turn_id: Some(kept_turn),
                },
            ],
        );

        let history = read_canonical_history(&dir.path().join("rollout.jsonl")).expect("read");
        assert_eq!(history.items.len(), 1);
        assert_eq!(history.items[0].state, ItemState::Completed);
        assert!(
            matches!(&history.items[0].item, devo_protocol::native::item::Item::AssistantMessage { text, .. } if text == "kept")
        );
    }

    #[test]
    fn truncated_final_line_is_tolerated() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item = assistant_item_envelope(session_id, turn_id, 1, "ok");
        let mut text = String::new();
        text.push_str(
            &serde_json::to_string(&RolloutLine::Item {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: item.updated_at,
                item,
            })
            .expect("serialize"),
        );
        text.push('\n');
        text.push_str(r#"{"v":2,"kind":"item","timestamp":"2026"#);
        std::fs::write(dir.path().join("rollout.jsonl"), text).expect("write fixture");

        let history = read_canonical_history(&dir.path().join("rollout.jsonl")).expect("read");
        assert_eq!(history.items.len(), 1);
    }

    #[test]
    fn append_after_read_snapshot_does_not_turn_truncated_tail_into_middle_damage() {
        let item = assistant_item_envelope(SessionId::new(), TurnId::new(), 1, "ok");
        let valid_line = serde_json::to_string(&RolloutLine::Item {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: item.updated_at,
            item,
        })
        .expect("serialize valid line");
        let truncated = r#"{"v":2,"kind":"item""#;
        let snapshot = format!("{valid_line}\n{truncated}");
        let snapshot_len = snapshot.len() as u64;
        let appended = format!("{snapshot}\n{valid_line}\n");
        let mut parsed = Vec::new();

        let result = visit_rollout_lines(
            std::io::Cursor::new(appended.as_bytes()).take(snapshot_len),
            |index, raw, is_last| {
                match parse_rollout_line(raw) {
                    Ok(_) => parsed.push(raw.to_string()),
                    Err(RolloutLineReadError::TruncatedTail) if is_last => {}
                    Err(error) => {
                        return Err(HistoryReadError::DamagedLine {
                            line_index: index,
                            error,
                        });
                    }
                }
                Ok(())
            },
        );

        assert!(result.is_ok());
        assert_eq!(parsed, vec![valid_line]);
    }

    #[test]
    fn streamed_history_reader_matches_full_projection() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let now = fixture_now();
        let mut turn = native_turn(
            devo_protocol::native::turn::TurnStatus::Completed,
            turn_id,
            1,
        );
        turn.session_id = session_id;
        turn.completed_at = Some(now);
        let item = assistant_item_envelope(session_id, turn_id, 1, "history");
        let lines = vec![
            RolloutLine::Turn {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: now,
                turn: Box::new(turn),
                extras: None,
            },
            RolloutLine::Item {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: now,
                item: item.clone(),
            },
            RolloutLine::Internal {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: now,
                session_id,
                turn_id: None,
                seq: 0,
                entry: InternalRecord::SessionLeaf {
                    epoch: 1,
                    leaf_id: Some(item.id),
                },
            },
            RolloutLine::Internal {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: now,
                session_id,
                turn_id: None,
                seq: 0,
                entry: InternalRecord::TreeEdge {
                    child_id: item.id,
                    parent_id: None,
                },
            },
        ];
        let mut expected = CanonicalHistory::default();
        for line in &lines {
            apply_rollout_line(&mut expected, line.clone());
        }
        let path = dir.path().join("rollout.jsonl");
        write_lines(&path, &lines);

        let actual = read_canonical_history(&path).expect("read streamed history");
        assert_eq!(actual, expected);
    }

    #[test]
    fn truncated_nonfinal_line_is_not_tolerated_before_blank_or_data() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let item = assistant_item_envelope(SessionId::new(), TurnId::new(), 1, "ok");
        let valid_line = serde_json::to_string(&RolloutLine::Item {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: item.updated_at,
            item,
        })
        .expect("serialize valid line");
        let truncated = r#"{"v":2,"kind":"item","timestamp":"2026""#;
        let path = dir.path().join("rollout.jsonl");
        for suffix in ["\n\n".to_string(), format!("\n{valid_line}\n")] {
            std::fs::write(&path, format!("{truncated}{suffix}")).expect("write fixture");
            let error = read_canonical_history(&path).expect_err("nonfinal tail must fail");
            assert!(matches!(
                error,
                HistoryReadError::DamagedLine {
                    line_index: 0,
                    error: RolloutLineReadError::TruncatedTail,
                }
            ));
        }
    }

    #[test]
    fn damaged_middle_line_fails_closed() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item = assistant_item_envelope(session_id, turn_id, 1, "ok");
        let mut text = String::new();
        text.push_str(
            &serde_json::to_string(&RolloutLine::Item {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: item.updated_at,
                item,
            })
            .expect("serialize"),
        );
        text.push('\n');
        text.push_str("{\"v\":2,\"kind\":\"nope\"}\n");
        std::fs::write(dir.path().join("rollout.jsonl"), text).expect("write fixture");

        let error = read_canonical_history(&dir.path().join("rollout.jsonl"))
            .expect_err("damaged line must fail");
        assert!(matches!(error, HistoryReadError::DamagedLine { .. }));
    }

    /// Verifies: field-level settings lines fold into the canonical session
    /// snapshot (last line per field wins) and bump its version (DD-4).
    #[test]
    fn session_settings_lines_fold_into_canonical_session() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let now = Utc.with_ymd_and_hms(2026, 8, 2, 12, 0, 0).unwrap();
        let record = crate::conversation::SessionRecord {
            id: session_id,
            rollout_path: dir.path().join("rollout.jsonl"),
            created_at: now,
            updated_at: now,
            last_activity_at: Some(now),
            source: "cli".into(),
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "test".into(),
            model: Some("test-model".into()),
            model_binding_id: None,
            reasoning_effort_selection: None,
            cwd: dir.path().to_path_buf(),
            additional_directories: Vec::new(),
            cli_version: "test".into(),
            title: None,
            title_state: crate::conversation::SessionTitleState::Unset,
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            effective_context_window: None,
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id: None,
            fork_from_id: None,
            fork_at_turn_id: None,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
            schema_version: 2,
        };
        write_lines(
            &dir.path().join("rollout.jsonl"),
            &[
                RolloutLine::SessionMeta {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    session: Box::new(
                        native_session_from_record(&record).expect("convert session record"),
                    ),
                    extras: None,
                },
                settings_line(
                    now,
                    session_id,
                    SessionSettingsField::PermissionPreset,
                    serde_json::to_value(devo_protocol::PermissionPreset::FullAccess)
                        .expect("serialize preset"),
                    1,
                ),
                settings_line(
                    now,
                    session_id,
                    SessionSettingsField::SandboxProfile,
                    serde_json::Value::String("strict".into()),
                    2,
                ),
                settings_line(
                    now,
                    session_id,
                    SessionSettingsField::ReasoningEffortSelection,
                    serde_json::to_value(Some("high".to_string())).expect("serialize effort"),
                    3,
                ),
            ],
        );

        let history = read_canonical_history(&dir.path().join("rollout.jsonl")).expect("read");
        let session = history.session.expect("session");
        assert_eq!(
            session.settings.permission_profile,
            devo_protocol::native::model::PermissionProfile::FullAccess
        );
        assert_eq!(session.settings.sandbox_profile.as_deref(), Some("strict"));
        assert_eq!(session.settings.reasoning_effort.as_deref(), Some("high"));
        // Three settings epochs (1, 2, 3) raise the SessionMeta version (1)
        // to 4.
        assert_eq!(session.version, 4);
    }

    /// Toggle keywords and level labels both survive the fold, and the raw
    /// literal is normalized on the way in so a stored selection compares
    /// equal to the same selection arriving in a patch.
    #[test]
    fn settings_fold_keeps_toggle_keywords_and_normalizes() {
        let fold_one = |raw: &str| -> Option<String> {
            let dir = tempfile::TempDir::new().expect("temp dir");
            let session_id = SessionId::new();
            let now = Utc.with_ymd_and_hms(2026, 8, 2, 12, 0, 0).unwrap();
            let record = crate::conversation::SessionRecord {
                id: session_id,
                rollout_path: dir.path().join("rollout.jsonl"),
                created_at: now,
                updated_at: now,
                last_activity_at: Some(now),
                source: "cli".into(),
                agent_nickname: None,
                agent_role: None,
                agent_path: None,
                model_provider: "test".into(),
                model: Some("test-model".into()),
                model_binding_id: None,
                reasoning_effort_selection: None,
                cwd: dir.path().to_path_buf(),
                additional_directories: Vec::new(),
                cli_version: "test".into(),
                title: None,
                title_state: crate::conversation::SessionTitleState::Unset,
                sandbox_policy: "workspace-write".into(),
                approval_mode: "on-request".into(),
                effective_context_window: None,
                tokens_used: 0,
                first_user_message: None,
                archived_at: None,
                git_sha: None,
                git_branch: None,
                git_origin_url: None,
                parent_session_id: None,
                fork_from_id: None,
                fork_at_turn_id: None,
                session_context: None,
                latest_turn_context: None,
                collaboration_mode: None,
                permission_preset: None,
                schema_version: 2,
            };
            write_lines(
                &dir.path().join("rollout.jsonl"),
                &[
                    RolloutLine::SessionMeta {
                        v: ROLLOUT_FORMAT_VERSION,
                        timestamp: now,
                        session: Box::new(
                            native_session_from_record(&record).expect("convert session record"),
                        ),
                        extras: None,
                    },
                    settings_line(
                        now,
                        session_id,
                        SessionSettingsField::ReasoningEffortSelection,
                        serde_json::to_value(Some(raw.to_string())).expect("serialize effort"),
                        1,
                    ),
                ],
            );
            let history =
                read_canonical_history(&dir.path().join("rollout.jsonl")).expect("read history");
            history.session.expect("session").settings.reasoning_effort
        };

        assert_eq!(fold_one("enabled").as_deref(), Some("on"));
        assert_eq!(fold_one("disabled").as_deref(), Some("off"));
        assert_eq!(fold_one(" High ").as_deref(), Some("high"));
    }

    /// The filtered meta reader agrees with the full canonical read on the
    /// latest header (last meta wins); a decoy line that merely contains the
    /// tag substring parses and is discarded, and a truncated crash tail
    /// does not fabricate a header.
    #[test]
    fn session_meta_reader_matches_canonical_history_and_ignores_decoys() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap();
        let record = crate::conversation::SessionRecord {
            id: session_id,
            rollout_path: dir.path().join("rollout.jsonl"),
            created_at: now,
            updated_at: now,
            last_activity_at: Some(now),
            source: "cli".into(),
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "test".into(),
            model: Some("test-model".into()),
            model_binding_id: None,
            reasoning_effort_selection: None,
            cwd: dir.path().to_path_buf(),
            additional_directories: Vec::new(),
            cli_version: "test".into(),
            title: None,
            title_state: crate::conversation::SessionTitleState::Unset,
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            effective_context_window: None,
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id: None,
            fork_from_id: None,
            fork_at_turn_id: None,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
            schema_version: 2,
        };
        let first = crate::conversation::SessionRecord {
            model: Some("first-model".into()),
            ..record.clone()
        };
        let latest = crate::conversation::SessionRecord {
            model: Some("latest-model".into()),
            ..record
        };
        let turn_id = TurnId::new();
        write_lines(
            &dir.path().join("rollout.jsonl"),
            &[
                RolloutLine::SessionMeta {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    session: Box::new(
                        native_session_from_record(&first).expect("convert first record"),
                    ),
                    extras: None,
                },
                // Decoy: the message text is exactly the tag substring, so
                // the line passes the pre-filter and must be discarded by
                // the typed match, not projected as a header.
                RolloutLine::Item {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    item: assistant_item_envelope(session_id, turn_id, 1, "sessionMeta"),
                },
                RolloutLine::SessionMeta {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    session: Box::new(
                        native_session_from_record(&latest).expect("convert latest record"),
                    ),
                    extras: None,
                },
                // Field-level settings and title folds must reach the header
                // exactly as the canonical reader applies them.
                settings_line(
                    now,
                    session_id,
                    SessionSettingsField::ReasoningEffortSelection,
                    serde_json::to_value(Some("high".to_string())).expect("serialize effort"),
                    1,
                ),
                RolloutLine::SessionTitleUpdated {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    session_id,
                    title: "Folded title".into(),
                    previous_title: None,
                },
            ],
        );
        let path = dir.path().join("rollout.jsonl");

        let meta = read_rollout_session_meta(&path)
            .expect("read meta")
            .expect("meta header");
        assert_eq!(meta.model.model, "latest-model");
        assert_eq!(meta.settings.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(meta.title.as_deref(), Some("Folded title"));
        let history = read_canonical_history(&path).expect("canonical history");
        assert_eq!(Some(meta), history.session);

        // A truncated final meta line is a crash tail: ignored, like the
        // canonical reader tolerates it.
        let mut crash_tail = std::fs::read_to_string(&path).expect("read fixture");
        crash_tail.push_str("{\"v\":2,\"kind\":\"sessionMeta\",\"session\":{");
        std::fs::write(&path, crash_tail).expect("append crash tail");
        let meta = read_rollout_session_meta(&path)
            .expect("read meta with crash tail")
            .expect("meta header");
        assert_eq!(meta.model.model, "latest-model");
    }

    #[test]
    fn session_leaf_reader_matches_canonical_history_and_ignores_decoys() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap();
        let stale_leaf = ItemId::new();
        let final_leaf = ItemId::new();
        let leaf_line = |epoch, leaf_id| RolloutLine::Internal {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: now,
            session_id,
            turn_id: None,
            seq: 0,
            entry: InternalRecord::SessionLeaf { epoch, leaf_id },
        };
        write_lines(
            &dir.path().join("rollout.jsonl"),
            &[
                leaf_line(4, Some(stale_leaf)),
                // A non-leaf payload can contain the tag text; it must not
                // affect the folded tip after typed dispatch.
                RolloutLine::Item {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    item: assistant_item_envelope(session_id, turn_id, 1, "sessionLeaf"),
                },
                leaf_line(3, Some(stale_leaf)),
                // Equal epochs are last-write-wins, matching apply_rollout_line.
                leaf_line(4, Some(final_leaf)),
            ],
        );
        let path = dir.path().join("rollout.jsonl");
        let history = read_canonical_history(&path).expect("canonical history");
        let expected = (history.leaf_id, history.leaf_epoch);
        assert_eq!(expected, (Some(final_leaf), 4));
        assert_eq!(
            read_rollout_session_leaf(&path).expect("read leaf"),
            expected
        );

        let mut crash_tail = std::fs::read_to_string(&path).expect("read fixture");
        crash_tail.push_str("{\"v\":2,\"kind\":\"internal\",\"entry\":{\"type\":\"sessionLeaf\"");
        std::fs::write(&path, crash_tail).expect("append crash tail");
        assert_eq!(
            read_rollout_session_leaf(&path).expect("read leaf with tail"),
            expected
        );
    }
    fn session_record_for(
        id: SessionId,
        parent: Option<SessionId>,
        dir: &Path,
        now: chrono::DateTime<Utc>,
    ) -> crate::conversation::SessionRecord {
        crate::conversation::SessionRecord {
            id,
            rollout_path: dir.join("rollout.jsonl"),
            created_at: now,
            updated_at: now,
            last_activity_at: Some(now),
            source: "cli".into(),
            agent_nickname: parent.map(|_| "child".to_string()),
            agent_role: parent.map(|_| "default".to_string()),
            agent_path: parent.map(|_| "root/child".to_string()),
            model_provider: "test".into(),
            model: Some("test-model".into()),
            model_binding_id: None,
            reasoning_effort_selection: None,
            cwd: dir.to_path_buf(),
            additional_directories: Vec::new(),
            cli_version: "test".into(),
            title: None,
            title_state: crate::conversation::SessionTitleState::Unset,
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            effective_context_window: None,
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id: parent,
            fork_from_id: None,
            fork_at_turn_id: None,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
            schema_version: 2,
        }
    }

    fn session_meta_line(
        record: &crate::conversation::SessionRecord,
        now: chrono::DateTime<Utc>,
    ) -> RolloutLine {
        RolloutLine::SessionMeta {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: now,
            session: Box::new(native_session_from_record(record).expect("convert session record")),
            extras: None,
        }
    }

    fn native_turn_for(
        session_id: SessionId,
        status: devo_protocol::native::turn::TurnStatus,
        id: TurnId,
        sequence: u32,
    ) -> devo_protocol::native::turn::Turn {
        let mut turn = native_turn(status, id, sequence);
        turn.session_id = session_id;
        turn
    }

    #[test]
    fn shared_journal_scoped_readers_isolate_sessions() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("rollout.jsonl");
        let now = fixture_now();
        let root = SessionId::new();
        let child = SessionId::new();
        let root_turn = TurnId::new();
        let child_turn = TurnId::new();
        let root_item = assistant_item_envelope(root, root_turn, 1, "root text");
        let child_item = assistant_item_envelope(child, child_turn, 1, "child text");
        let root_leaf = ItemId::new();
        let child_leaf = ItemId::new();
        let root_edge = ItemId::new();
        let child_edge = ItemId::new();
        let root_record = session_record_for(root, None, dir.path(), now);
        let child_record = session_record_for(child, Some(root), dir.path(), now);
        let leaf_line = |session_id, epoch, leaf_id| RolloutLine::Internal {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: now,
            session_id,
            turn_id: None,
            seq: 0,
            entry: InternalRecord::SessionLeaf { epoch, leaf_id },
        };
        let edge_line = |session_id, child_id| RolloutLine::Internal {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: now,
            session_id,
            turn_id: None,
            seq: 0,
            entry: InternalRecord::TreeEdge {
                child_id,
                parent_id: None,
            },
        };
        write_lines(
            &path,
            &[
                session_meta_line(&root_record, now),
                session_meta_line(&child_record, now),
                RolloutLine::Turn {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    turn: Box::new(native_turn_for(
                        root,
                        devo_protocol::native::turn::TurnStatus::Completed,
                        root_turn,
                        1,
                    )),
                    extras: None,
                },
                RolloutLine::Turn {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    turn: Box::new(native_turn_for(
                        child,
                        devo_protocol::native::turn::TurnStatus::Completed,
                        child_turn,
                        1,
                    )),
                    extras: None,
                },
                RolloutLine::Item {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    item: root_item.clone(),
                },
                RolloutLine::Item {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    item: child_item.clone(),
                },
                RolloutLine::SessionTitleUpdated {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    session_id: root,
                    title: "root title".into(),
                    previous_title: None,
                },
                RolloutLine::SessionTitleUpdated {
                    v: ROLLOUT_FORMAT_VERSION,
                    timestamp: now,
                    session_id: child,
                    title: "child title".into(),
                    previous_title: None,
                },
                settings_line(
                    now,
                    root,
                    SessionSettingsField::Model,
                    serde_json::json!("root-model"),
                    1,
                ),
                settings_line(
                    now,
                    child,
                    SessionSettingsField::Model,
                    serde_json::json!("child-model"),
                    1,
                ),
                leaf_line(root, 1, Some(root_leaf)),
                leaf_line(child, 9, Some(child_leaf)),
                edge_line(root, root_edge),
                edge_line(child, child_edge),
            ],
        );

        let root_history = read_canonical_history_for_session(&path, &root).expect("root history");
        assert_eq!(root_history.session.as_ref().map(|s| s.id), Some(root));
        assert_eq!(
            root_history.session.as_ref().and_then(|s| s.title.clone()),
            Some("root title".into())
        );
        assert_eq!(root_history.turns.len(), 1);
        assert_eq!(root_history.turns[0].id, root_turn);
        assert_eq!(root_history.items.len(), 1);
        assert_eq!(root_history.items[0].session_id, root);
        assert_eq!(
            (root_history.leaf_id, root_history.leaf_epoch),
            (Some(root_leaf), 1)
        );
        assert_eq!(root_history.tree_edges.get(&root_edge), Some(&None));
        assert!(!root_history.tree_edges.contains_key(&child_edge));

        let child_history =
            read_canonical_history_for_session(&path, &child).expect("child history");
        assert_eq!(child_history.session.as_ref().map(|s| s.id), Some(child));
        assert_eq!(child_history.turns.len(), 1);
        assert_eq!(child_history.turns[0].id, child_turn);
        assert_eq!(child_history.items.len(), 1);
        assert_eq!(child_history.items[0].session_id, child);
        assert_eq!(
            (child_history.leaf_id, child_history.leaf_epoch),
            (Some(child_leaf), 9)
        );
        assert_eq!(child_history.tree_edges.get(&child_edge), Some(&None));
        assert!(!child_history.tree_edges.contains_key(&root_edge));

        assert_eq!(
            read_rollout_session_meta_for_session(&path, &root)
                .expect("root meta")
                .map(|s| s.id),
            Some(root)
        );
        assert_eq!(
            read_rollout_session_meta_for_session(&path, &child)
                .expect("child meta")
                .map(|s| s.id),
            Some(child)
        );
        assert_eq!(
            read_rollout_session_leaf_for_session(&path, &root).expect("root leaf"),
            (Some(root_leaf), 1)
        );
        assert_eq!(
            read_rollout_session_leaf_for_session(&path, &child).expect("child leaf"),
            (Some(child_leaf), 9)
        );
    }

    #[test]
    fn scoped_reader_reports_session_deleted_tombstone() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("rollout.jsonl");
        let now = fixture_now();
        let root = SessionId::new();
        let child = SessionId::new();
        let root_record = session_record_for(root, None, dir.path(), now);
        let child_record = session_record_for(child, Some(root), dir.path(), now);
        let mut lines = vec![
            session_meta_line(&root_record, now),
            session_meta_line(&child_record, now),
        ];
        lines.push(RolloutLine::Internal {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: now,
            session_id: child,
            turn_id: None,
            seq: 0,
            entry: InternalRecord::SessionDeleted {
                schema_version: 1,
                deleted_at: now,
            },
        });
        write_lines(&path, &lines);

        let error = read_canonical_history_for_session(&path, &child)
            .expect_err("deleted child must not read");
        assert!(matches!(
            error,
            HistoryReadError::SessionDeleted { session_id, deleted_at }
                if session_id == child && deleted_at == now
        ));
        assert!(
            read_canonical_history_for_session(&path, &root).is_ok(),
            "sibling tombstone must not affect the root"
        );
        assert!(matches!(
            read_rollout_session_meta_for_session(&path, &child),
            Err(HistoryReadError::SessionDeleted { .. })
        ));
        assert!(matches!(
            read_rollout_session_leaf_for_session(&path, &child),
            Err(HistoryReadError::SessionDeleted { .. })
        ));
        // Unscoped whole-file scans skip the tombstone rather than failing.
        assert!(read_canonical_history(&path).is_ok());
    }
}
