//! Incremental canonical-journal replay. Never skip a locally appended row:
//! another writer/checkpoint may have landed beside it before the next read.
use anyhow::{Context, Result};
use devo_core::{RolloutLine, RolloutLineReadError, parse_rollout_line};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
    time::SystemTime,
};

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(windows)]
    identity: u64,
}
impl Stamp {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            },
            #[cfg(windows)]
            identity: {
                use std::os::windows::fs::MetadataExt;
                metadata.creation_time()
            },
        }
    }
    fn same_file(&self, other: &Self) -> bool {
        #[cfg(any(unix, windows))]
        {
            self.identity == other.identity
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = other;
            true
        }
    }
}

#[derive(Default)]
pub(super) struct JournalCursor {
    stamp: Option<Stamp>,
    offset: u64,
}
impl JournalCursor {
    /// Returns bytes read, useful for verifying that steady-state calls do not
    /// reparse the accumulated transcript. A partial final row is retried later.
    pub(super) fn read_new(
        &mut self,
        path: &Path,
        mut visit: impl FnMut(RolloutLine),
    ) -> Result<u64> {
        let mut file =
            File::open(path).with_context(|| format!("open usage journal {}", path.display()))?;
        let stamp = Stamp::from_metadata(&file.metadata()?);
        if self.stamp.as_ref() == Some(&stamp) {
            return Ok(0);
        }
        let start = if self.stamp.as_ref().is_some_and(|old| {
            old.same_file(&stamp) && stamp.len > old.len && self.offset <= stamp.len
        }) {
            self.offset
        } else {
            0
        };
        file.seek(SeekFrom::Start(start))?;
        // Bound this read to the opened snapshot; a racing append belongs to
        // the next pass and cannot change crash-tail classification.
        let mut reader = BufReader::new(file.take(stamp.len - start));
        let mut offset = start;
        let mut bytes = 0;
        let mut line = Vec::new();
        loop {
            line.clear();
            let count = reader.read_until(b'\n', &mut line)?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
            let text = match std::str::from_utf8(&line) {
                Ok(text) => text,
                Err(error) if error.error_len().is_none() && reader.fill_buf()?.is_empty() => break,
                Err(error) => return Err(error).context("invalid UTF-8 inside usage journal"),
            };
            if !text.trim().is_empty() {
                match parse_rollout_line(text) {
                    Ok(line) => visit(line),
                    Err(RolloutLineReadError::TruncatedTail) => {
                        let mut rest = String::new();
                        bytes += reader.read_to_string(&mut rest)? as u64;
                        if rest.trim().is_empty() {
                            break;
                        }
                        anyhow::bail!("damaged usage journal {} at byte {offset}", path.display());
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("damaged usage journal {} at byte {offset}", path.display())
                        });
                    }
                }
            }
            offset += count as u64;
        }
        self.offset = offset;
        self.stamp = Some(stamp);
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::io::Write;

    #[test]
    fn append_reads_only_new_bytes_and_retries_partial_tail() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let path = temp.path();
        use devo_protocol::native::{
            ids::{ItemId, SessionId, TurnId},
            item::{Item, ItemEnvelope, ItemState},
        };
        let now = chrono::Utc::now();
        let history = RolloutLine::Item {
            v: 2,
            timestamp: now,
            item: ItemEnvelope {
                id: ItemId::new(),
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                seq: 1,
                revision: 1,
                created_at: now,
                updated_at: now,
                state: ItemState::Completed,
                item: Item::AssistantMessage {
                    text: "x".repeat(4 * 1024 * 1024),
                },
                parent_id: None,
            },
        };
        let history = format!("{}\n", serde_json::to_string(&history).unwrap());
        std::fs::write(path, &history).unwrap();
        let mut cursor = JournalCursor::default();
        assert_eq!(cursor.read_new(path, |_| {}).unwrap(), history.len() as u64);
        let mut append = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        append.write_all(b"\n\n").unwrap();
        assert_eq!(cursor.read_new(path, |_| {}).unwrap(), 2);
        assert_eq!(cursor.read_new(path, |_| {}).unwrap(), 0);
        append
            .write_all(b"{\"v\":2,\"kind\":\"sessionRollback\"")
            .unwrap();
        cursor.read_new(path, |_| {}).unwrap();
        let before = cursor.offset;
        append.write_all(b",\"timestamp\":\"2026-01-01T00:00:00Z\",\"sessionId\":\"session-test\",\"retainedTurnIds\":[],\"retainedItemIds\":[],\"latestTurnId\":null}\n").unwrap();
        let mut rows = 0;
        cursor.read_new(path, |_| rows += 1).unwrap();
        assert_eq!(rows, 1);
        assert!(cursor.offset > before);
    }
    #[test]
    fn partial_unicode_at_snapshot_tail_waits_for_the_remaining_bytes() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let line = format!(
            "{}\n",
            serde_json::to_string(&RolloutLine::SessionTitleUpdated {
                v: 2,
                timestamp: chrono::Utc::now(),
                session_id: devo_protocol::native::ids::SessionId::new(),
                title: "中文".into(),
                previous_title: None,
            })
            .unwrap()
        );
        let split = line.find('中').unwrap() + 1;
        std::fs::write(temp.path(), &line.as_bytes()[..split]).unwrap();
        let mut cursor = JournalCursor::default();
        assert_eq!(
            cursor
                .read_new(temp.path(), |_| panic!("partial row"))
                .unwrap(),
            split as u64
        );
        assert_eq!(cursor.offset, 0);
        std::fs::OpenOptions::new()
            .append(true)
            .open(temp.path())
            .unwrap()
            .write_all(&line.as_bytes()[split..])
            .unwrap();
        let mut count = 0;
        cursor.read_new(temp.path(), |_| count += 1).unwrap();
        assert_eq!(count, 1);
        assert_eq!(cursor.offset, line.len() as u64);
    }
}
