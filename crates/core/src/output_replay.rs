//! Restore output read capabilities from the owning session's durable history.

use std::io::BufRead;
use std::path::Path;

use devo_tools::output_store::OutputArtifact;

use crate::durable_execution::{ExecutionRecord, ExecutionReplay};
use crate::{InternalRecord, RolloutLine, RolloutLineReadError, parse_rollout_line};

/// Manifests alone are not capabilities. Only committed session references,
/// including explicitly inherited references, authorize restored artifact reads.
pub fn read_output_references(path: &Path) -> anyhow::Result<Vec<OutputArtifact>> {
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
                    entry:
                        InternalRecord::Execution {
                            record: record @ ExecutionRecord::OutputArtifacts { .. },
                        },
                    ..
                } = line
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
