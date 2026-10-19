//! Ignored manual probe for rollout-index refresh phase timing.
//!
//! Set `DEVO_INDEX_PERF_ROOT` to a stable server data root and run this test
//! explicitly. It reads rollout files but writes only to a temporary SQLite DB.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use super::{RolloutStore, read_rollout_index_fields, session_index_row_from_record};
use crate::db::Database;
use devo_core::SessionRecord;
use devo_protocol::native::ids::SessionId;

#[test]
#[ignore = "manual index perf probe; requires DEVO_INDEX_PERF_ROOT"]
fn rollout_index_refresh_phase_probe() -> anyhow::Result<()> {
    let root = std::env::var_os("DEVO_INDEX_PERF_ROOT")
        .map(PathBuf::from)
        .expect("set DEVO_INDEX_PERF_ROOT to a stable server data root");
    let store = RolloutStore::new(root, /*event_log*/ None);

    let discovery_started = Instant::now();
    let paths = store.rollout_paths()?;
    let discovery_elapsed = discovery_started.elapsed();

    let mut total_bytes = 0u64;
    for path in &paths {
        total_bytes += std::fs::metadata(path)?.len();
    }

    let scan_started = Instant::now();
    let mut scanned: Vec<(PathBuf, SessionRecord, chrono::DateTime<chrono::Utc>)> =
        Vec::with_capacity(paths.len());
    let mut skipped = 0usize;
    for path in &paths {
        match read_rollout_index_fields(path) {
            Ok((record, last_activity_at)) => {
                scanned.push((path.clone(), record, last_activity_at));
            }
            Err(_) => skipped += 1,
        }
    }
    let scan_elapsed = scan_started.elapsed();
    let successful_files = scanned.len();

    let dedup_started = Instant::now();
    let mut canonical =
        HashMap::<SessionId, (chrono::DateTime<chrono::Utc>, PathBuf, SessionRecord)>::new();
    for (path, record, last_activity_at) in scanned {
        let replace = match canonical.get(&record.id) {
            Some((existing_activity, _, _)) => last_activity_at > *existing_activity,
            None => true,
        };
        if replace {
            canonical.insert(record.id, (last_activity_at, path, record));
        }
    }
    let dedup_elapsed = dedup_started.elapsed();
    let canonical_count = canonical.len();

    let conversion_started = Instant::now();
    let rows: Vec<_> = canonical
        .values()
        .map(|(last_activity_at, path, record)| {
            (
                session_index_row_from_record(record, *last_activity_at),
                path.clone(),
            )
        })
        .collect();
    let conversion_elapsed = conversion_started.elapsed();

    let temp_dir = tempfile::tempdir()?;
    let db = Database::open(temp_dir.path().join("index.sqlite"))?;
    let upsert_started = Instant::now();
    for (row, path) in rows {
        db.upsert_rollout_index_session(row, Some(path.as_path()))?;
    }
    let upsert_elapsed = upsert_started.elapsed();

    let repeat_started = Instant::now();
    store.index_rollout_metadata(&db)?;
    let repeat_elapsed = repeat_started.elapsed();

    println!(
        "[index-probe] files={} bytes={} scanned={} skipped={} canonical={} duplicates={} discovery={:.1?} scan={:.1?} dedup={:.1?} row_conversion={:.1?} upsert={:.1?} repeat_full={:.1?}",
        paths.len(),
        total_bytes,
        successful_files,
        skipped,
        canonical_count,
        successful_files.saturating_sub(canonical_count),
        discovery_elapsed,
        scan_elapsed,
        dedup_elapsed,
        conversion_elapsed,
        upsert_elapsed,
        repeat_elapsed,
    );

    drop(db);
    drop(temp_dir);
    Ok(())
}
