//! Regression coverage for histories larger than the former 4 MiB entry cap.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use devo_core::{RolloutLine, read_canonical_history_for_session};
use devo_protocol::native::ids::{ItemId, SessionId, TurnId};
use devo_protocol::native::item::{Item, ItemEnvelope, ItemState};
use pretty_assertions::assert_eq;
use tokio::sync::Barrier;

use super::{FileVersion, HistoryCache, MAX_CACHE_ENTRIES, MAX_TOTAL_CACHE_BYTES};

fn write_history(path: &Path, session_id: SessionId, count: usize, text_bytes: usize) {
    let mut writer = BufWriter::new(fs::File::create(path).expect("create rollout"));
    let now = Utc::now();
    let turn_id = TurnId::new();
    for index in 0..count {
        let line = RolloutLine::Item {
            v: devo_core::ROLLOUT_FORMAT_VERSION,
            timestamp: now,
            item: ItemEnvelope {
                id: ItemId::new(),
                session_id,
                turn_id,
                seq: index as u64 + 1,
                revision: 1,
                created_at: now,
                updated_at: now,
                state: ItemState::Completed,
                item: Item::AssistantMessage {
                    text: "x".repeat(text_bytes),
                },
                parent_id: None,
            },
        };
        serde_json::to_writer(&mut writer, &line).expect("serialize item");
        writer.write_all(b"\n").expect("write newline");
    }
    writer.flush().expect("flush rollout");
}

#[tokio::test]
async fn large_history_pages_share_projection_and_append_invalidates() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("session.jsonl");
    let session_id = SessionId::new();
    write_history(&path, session_id, 6000, 1024);
    let bytes = fs::metadata(&path).expect("metadata").len();
    assert!(bytes > 4 * 1024 * 1024 && bytes <= MAX_TOTAL_CACHE_BYTES);
    let expected =
        read_canonical_history_for_session(&path, &session_id).expect("expected history");
    let cache = HistoryCache::default();
    let first = cache.load(session_id, path.clone()).await.expect("history");
    // Each 200-item page reaches the same load boundary as the Native handler.
    for _ in 0..30 {
        let page_history = cache.load(session_id, path.clone()).await.expect("page");
        assert!(Arc::ptr_eq(&first, &page_history));
    }
    assert_eq!(first.as_ref(), &expected);
    assert_eq!(cache.parse_count(), 1);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open rollout")
        .write_all(b"\n")
        .expect("append");
    let changed = cache.load(session_id, path).await.expect("changed history");
    assert!(!Arc::ptr_eq(&first, &changed));
    assert_eq!(changed.as_ref(), &expected);
    assert_eq!(cache.parse_count(), 2);
}

#[tokio::test]
async fn concurrent_large_history_reads_share_one_parse() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("session.jsonl");
    let session_id = SessionId::new();
    write_history(&path, session_id, 1, 5 * 1024 * 1024);
    let cache = Arc::new(HistoryCache::default());
    let barrier = Arc::new(Barrier::new(5));
    let mut readers = Vec::new();
    for _ in 0..4 {
        let cache = Arc::clone(&cache);
        let barrier = Arc::clone(&barrier);
        let path = path.clone();
        readers.push(tokio::spawn(async move {
            barrier.wait().await;
            cache.load(session_id, path).await.expect("history")
        }));
    }
    barrier.wait().await;
    let first = readers.pop().expect("reader").await.expect("joined");
    for reader in readers {
        assert!(Arc::ptr_eq(&first, &reader.await.expect("joined")));
    }
    assert_eq!(cache.parse_count(), 1);
}

#[tokio::test]
async fn history_above_total_budget_is_not_retained() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("session.jsonl");
    let session_id = SessionId::new();
    write_history(&path, session_id, 1, MAX_TOTAL_CACHE_BYTES as usize);
    assert!(fs::metadata(&path).expect("metadata").len() > MAX_TOTAL_CACHE_BYTES);
    let cache = HistoryCache::default();
    let first = cache.load(session_id, path.clone()).await.expect("history");
    let second = cache.load(session_id, path).await.expect("history");
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(first, second);
    let state = cache.state.lock().await;
    assert_eq!(
        (cache.parse_count(), state.entries.len(), state.total_bytes),
        (2, 0, 0)
    );
}

#[tokio::test]
async fn large_entries_obey_total_budget_and_lru_eviction() {
    let cache = HistoryCache::default();
    let version = FileVersion {
        len: 6 * 1024 * 1024,
        modified: std::time::SystemTime::UNIX_EPOCH,
    };
    let ids = [SessionId::new(), SessionId::new(), SessionId::new()];
    let path = Path::new("session.jsonl");
    // Synthetic versions isolate accounting from filesystem/parser costs.
    for id in &ids[..2] {
        cache
            .insert(*id, path.into(), version, Arc::default())
            .await;
    }
    assert!(cache.cached(ids[0], path, version).await.is_some());
    cache
        .insert(ids[2], path.into(), version, Arc::default())
        .await;
    assert!(cache.cached(ids[0], path, version).await.is_some());
    assert!(cache.cached(ids[1], path, version).await.is_none());
    assert!(cache.cached(ids[2], path, version).await.is_some());
    {
        let state = cache.state.lock().await;
        assert_eq!(
            (state.entries.len(), state.total_bytes),
            (2, 12 * 1024 * 1024)
        );
    }
    // One exactly-budget-sized entry is allowed and evicts all others.
    let full_version = FileVersion {
        len: MAX_TOTAL_CACHE_BYTES,
        ..version
    };
    cache
        .insert(ids[1], path.into(), full_version, Arc::default())
        .await;
    let state = cache.state.lock().await;
    assert_eq!(
        (state.entries.len(), state.total_bytes),
        (1, MAX_TOTAL_CACHE_BYTES)
    );
}

#[tokio::test]
async fn small_entries_still_obey_entry_count_limit() {
    let cache = HistoryCache::default();
    let path = Path::new("session.jsonl");
    let version = FileVersion {
        len: 1,
        modified: std::time::SystemTime::UNIX_EPOCH,
    };
    let first = SessionId::new();
    cache
        .insert(first, path.into(), version, Arc::default())
        .await;
    for _ in 0..MAX_CACHE_ENTRIES {
        cache
            .insert(SessionId::new(), path.into(), version, Arc::default())
            .await;
    }
    assert!(cache.cached(first, path, version).await.is_none());
    let state = cache.state.lock().await;
    assert_eq!(
        (state.entries.len(), state.total_bytes),
        (MAX_CACHE_ENTRIES, MAX_CACHE_ENTRIES as u64)
    );
}

#[tokio::test]
#[ignore = "manual large-history load benchmark; no wall-clock CI assertions"]
async fn benchmark_large_history_page_loads() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("session.jsonl");
    let session_id = SessionId::new();
    write_history(&path, session_id, 6000, 1024);
    let bytes = fs::metadata(&path).expect("metadata").len();
    // Above the former entry cap, the old cache reparsed on every load.
    let start = Instant::now();
    for _ in 0..30 {
        let history = read_canonical_history_for_session(&path, &session_id).expect("history");
        assert_eq!(history.items.len(), 6000);
    }
    let uncached = start.elapsed();
    let cache = HistoryCache::default();
    let start = Instant::now();
    for _ in 0..30 {
        let history = cache.load(session_id, path.clone()).await.expect("history");
        assert_eq!(history.items.len(), 6000);
    }
    let cached = start.elapsed();
    assert_eq!(cache.parse_count(), 1);
    eprintln!(
        "large history: journal_bytes={bytes}, loads=30, uncached={uncached:?}, cached={cached:?}, cached_parses={}",
        cache.parse_count()
    );
}
