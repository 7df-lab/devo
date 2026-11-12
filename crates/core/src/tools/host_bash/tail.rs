//! Bounded recent bytes for host bash, independent of the capped progress stream.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const CAPACITY: usize = 256 * 1024;

/// Shared rolling output retained directly by pipe readers, without queued deltas.
#[derive(Clone, Debug, Default)]
pub struct OutputTail(Arc<Mutex<VecDeque<u8>>>);

impl OutputTail {
    pub(crate) fn append(&self, bytes: &[u8]) {
        let mut buffer = self.0.lock().unwrap();
        let bytes = &bytes[bytes.len().saturating_sub(CAPACITY)..];
        let discard = (buffer.len() + bytes.len()).saturating_sub(CAPACITY);
        buffer.drain(..discard);
        buffer.extend(bytes);
    }

    pub(crate) fn snapshot(&self, max_bytes: usize) -> String {
        let buffer = self.0.lock().unwrap();
        let bytes: Vec<_> = buffer
            .iter()
            .skip(buffer.len().saturating_sub(max_bytes))
            .copied()
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn retains_latest_bytes_after_multiple_overflows() {
        let tail = OutputTail::default();
        tail.append(&vec![b'a'; CAPACITY * 2]);
        tail.append(b"latest\n");
        assert_eq!(
            tail.snapshot(CAPACITY),
            format!("{}latest\n", "a".repeat(CAPACITY - 7))
        );
        tail.append(&vec![b'b'; CAPACITY * 2]);
        assert_eq!(tail.snapshot(CAPACITY), "b".repeat(CAPACITY));
        assert_eq!(tail.0.lock().unwrap().len(), CAPACITY);
    }

    #[test]
    fn preserves_utf8_split_across_reads_and_handles_invalid_bytes() {
        let tail = OutputTail::default();
        tail.append(&[0xe4, 0xb8]);
        tail.append(&[0xad, b'\n']);
        assert_eq!(tail.snapshot(CAPACITY), "中\n");
        tail.append(&[0xff]);
        assert_eq!(tail.snapshot(CAPACITY), "中\n�");
    }
}
