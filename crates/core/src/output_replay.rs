//! Restore output read capabilities from the owning session's durable history.

use std::io::BufRead;
use std::path::Path;

use devo_tools::output_store::OutputArtifact;

use crate::durable_execution::{ExecutionRecord, ExecutionReplay};
use crate::{InternalRecord, RolloutLine, RolloutLineReadError, parse_rollout_line};

/// Manifests alone are not capabilities. Only committed session references,
/// including explicitly inherited references, authorize restored artifact reads.
pub fn read_output_references(path: &Path) -> anyhow::Result<Vec<OutputArtifact>> {
    read_output_references_scoped(path, /*session_id*/ None)
}

/// Restores output references owned by one logical session inside a journal.
///
/// A child sharing the root journal must not inherit the root's or a sibling's
/// output capabilities; use this form for turn restore, fork, and child delete.
/// The unscoped form remains for whole-tree GC.
pub fn read_output_references_for_session(
    path: &Path,
    session_id: &devo_protocol::native::ids::SessionId,
) -> anyhow::Result<Vec<OutputArtifact>> {
    read_output_references_scoped(path, Some(*session_id))
}

fn read_output_references_scoped(
    path: &Path,
    session_id: Option<devo_protocol::native::ids::SessionId>,
) -> anyhow::Result<Vec<OutputArtifact>> {
    let mut replay = ExecutionReplay::default();
    let mut lines = std::io::BufReader::new(std::fs::File::open(path)?)
        .lines()
        .peekable();
    while let Some(line) = lines.next() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match parse_rollout_line(&line) {
            Ok(line) => {
                if let RolloutLine::Internal {
                    session_id: owner,
                    entry:
                        InternalRecord::Execution {
                            record: record @ ExecutionRecord::OutputArtifacts { .. },
                        },
                    ..
                } = line
                    && session_id.is_none_or(|expected| owner == expected)
                {
                    replay.apply(&record)?;
                }
            }
            Err(RolloutLineReadError::TruncatedTail) => {
                let only_blank_remain = {
                    let mut blank = true;
                    while let Some(next) = lines.peek() {
                        match next {
                            Ok(line) if line.trim().is_empty() => {
                                let _ = lines.next();
                            }
                            Ok(_) => {
                                blank = false;
                                break;
                            }
                            Err(_) => {
                                blank = false;
                                break;
                            }
                        }
                    }
                    blank
                };
                if only_blank_remain {
                    break;
                }
                return Err(RolloutLineReadError::TruncatedTail.into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(replay.artifacts.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use devo_protocol::native::ids::{SessionId, TurnId};
    use devo_tools::output_store::CaptureState;

    fn artifact(id: &str, owner: &SessionId) -> OutputArtifact {
        OutputArtifact {
            id: id.into(),
            file_identity: format!("{id}-identity"),
            owner: owner.to_string(),
            call_id: format!("call-{id}"),
            path: PathBuf::from(format!("/tmp/outputs/{id}.txt")),
            bytes: 1,
            digest: "digest".into(),
            state: CaptureState::Complete,
        }
    }

    fn output_line(session_id: SessionId, artifact: OutputArtifact) -> RolloutLine {
        RolloutLine::Internal {
            v: 2,
            timestamp: chrono::Utc::now(),
            session_id,
            turn_id: Some(TurnId::new()),
            seq: 0,
            entry: InternalRecord::Execution {
                record: ExecutionRecord::OutputArtifacts {
                    artifacts: vec![artifact],
                },
            },
        }
    }

    #[test]
    fn scoped_output_references_do_not_cross_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("journal.jsonl");
        let root = SessionId::new();
        let child = SessionId::new();
        let root_artifact = artifact("root", &root);
        let child_artifact = artifact("child", &child);
        let lines = [
            output_line(root, root_artifact.clone()),
            output_line(child, child_artifact.clone()),
        ];
        let mut text = String::new();
        for line in &lines {
            text.push_str(&serde_json::to_string(line).expect("serialize"));
            text.push('\n');
        }
        std::fs::write(&path, text).expect("write journal");

        assert_eq!(
            read_output_references_for_session(&path, &root).expect("root refs"),
            vec![root_artifact]
        );
        assert_eq!(
            read_output_references_for_session(&path, &child).expect("child refs"),
            vec![child_artifact]
        );
        assert_eq!(read_output_references(&path).expect("all refs").len(), 2);
    }
}
