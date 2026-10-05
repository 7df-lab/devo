#![allow(dead_code)]

use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use devo_core::LegacyRolloutLine;
use devo_core::RolloutLine;
use devo_core::TurnWorkspaceChangeRecordedLine;
use devo_core::TurnWorkspaceCheckpointRecordedLine;
use devo_core::TurnWorkspaceRestoreCompletedLine;
use devo_core::TurnWorkspaceRestoreStartedLine;
use devo_core::item_record_from_native;
use devo_core::legacy_compaction_line_from_native;
use devo_core::legacy_lines_from_internal;
use devo_core::legacy_rollback_line_from_native;
use devo_core::legacy_title_line_from_native;
use devo_core::parse_rollout_line;
use devo_core::session_record_from_native;
use devo_core::turn_record_from_native;

/// Reads a rollout file into the legacy records inspected by integration tests.
/// Session/Turn/Item use the same direct record adapters as production replay;
/// only records still owned by the compatibility path become legacy lines.
pub fn read_rollout_records(path: &Path) -> Result<Vec<LegacyRolloutLine>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read rollout {}", path.display()))?;
    let mut out = Vec::new();
    for raw in text.lines().filter(|line| !line.trim().is_empty()) {
        let line =
            parse_rollout_line(raw).with_context(|| format!("parse line in {}", path.display()))?;
        match line {
            RolloutLine::SessionMeta {
                timestamp,
                session,
                extras,
                ..
            } => out.push(LegacyRolloutLine::SessionMeta(Box::new(
                devo_core::SessionMetaLine {
                    timestamp,
                    session: session_record_from_native(&session, extras.as_deref())?,
                },
            ))),
            RolloutLine::Turn {
                timestamp,
                turn,
                extras,
                ..
            } => out.push(LegacyRolloutLine::Turn(Box::new(devo_core::TurnLine {
                timestamp,
                turn: turn_record_from_native(&turn, extras.as_deref())?,
            }))),
            RolloutLine::Item { item, .. } => {
                if let Some(item) = item_record_from_native(&item)? {
                    out.push(LegacyRolloutLine::Item(Box::new(devo_core::ItemLine {
                        timestamp: item.timestamp,
                        item,
                    })));
                }
            }
            RolloutLine::WorkspaceCheckpoint {
                timestamp, record, ..
            } => out.push(LegacyRolloutLine::TurnWorkspaceCheckpointRecorded(
                Box::new(TurnWorkspaceCheckpointRecordedLine { timestamp, record }),
            )),
            RolloutLine::WorkspaceChange {
                timestamp, record, ..
            } => out.push(LegacyRolloutLine::TurnWorkspaceChangeRecorded(Box::new(
                TurnWorkspaceChangeRecordedLine { timestamp, record },
            ))),
            RolloutLine::WorkspaceRestoreStarted {
                timestamp, record, ..
            } => out.push(LegacyRolloutLine::TurnWorkspaceRestoreStarted(Box::new(
                TurnWorkspaceRestoreStartedLine { timestamp, record },
            ))),
            RolloutLine::WorkspaceRestoreCompleted {
                timestamp, record, ..
            } => out.push(LegacyRolloutLine::TurnWorkspaceRestoreCompleted(Box::new(
                TurnWorkspaceRestoreCompletedLine { timestamp, record },
            ))),
            RolloutLine::Internal {
                timestamp,
                session_id,
                turn_id,
                seq,
                entry,
                ..
            } => out.extend(legacy_lines_from_internal(
                timestamp,
                &session_id,
                turn_id.as_ref(),
                seq,
                &entry,
            )?),
            RolloutLine::SessionTitleUpdated {
                timestamp,
                session_id,
                title,
                previous_title,
                ..
            } => out.push(legacy_title_line_from_native(
                timestamp,
                &session_id,
                title,
                previous_title,
            )?),
            RolloutLine::CompactionSnapshot {
                timestamp,
                session_id,
                turn_id,
                summary_item_id,
                preserved_item_ids,
                context_occupancy,
                ..
            } => out.push(legacy_compaction_line_from_native(
                timestamp,
                &session_id,
                &turn_id,
                &summary_item_id,
                &preserved_item_ids,
                context_occupancy,
            )?),
            RolloutLine::SessionRollback {
                timestamp,
                session_id,
                retained_turn_ids,
                retained_item_ids,
                latest_turn_id,
                ..
            } => out.push(legacy_rollback_line_from_native(
                timestamp,
                &session_id,
                &retained_turn_ids,
                &retained_item_ids,
                latest_turn_id.as_ref(),
            )?),
        }
    }
    Ok(out)
}
