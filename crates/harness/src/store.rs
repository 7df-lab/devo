//! One read-modify-write transaction boundary for host CRUD and refinement.
//!
//! The sidecar lock is stable across atomic replacement and coordinates other
//! server processes too. Never hold it while waiting for permission or a model.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::{HarnessState, HarnessStateError};

/// Resolve existing symlinks, including a missing file under an existing parent.
pub fn canonical_harness_path(path: &Path) -> Result<PathBuf, HarnessStateError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    while !existing.try_exists()? {
        let name = existing.file_name().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid harness path")
        })?;
        missing.push(name.to_owned());
        existing = existing.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid harness parent")
        })?;
    }
    let mut canonical = fs::canonicalize(existing)?;
    for name in missing.into_iter().rev() {
        canonical.push(name);
    }
    Ok(canonical)
}

fn lock_store(path: &Path) -> Result<(PathBuf, fs::File), HarnessStateError> {
    let path = canonical_harness_path(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(path.with_extension("json.lock"))?;
    lock.lock_exclusive()?;
    Ok((path, lock))
}

/// Apply one operation against the latest state, with no stale snapshot overwrite.
/// The closure must be short and synchronous; errors leave the original intact.
pub fn transact_harness<T, E>(
    path: &Path,
    operation: impl FnOnce(&mut HarnessState) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<HarnessStateError>,
{
    let (path, _lock) = lock_store(path)?;
    let mut state = HarnessState::load(&path)?;
    let result = operation(&mut state)?;
    write_atomic(&state, &path)?;
    Ok(result)
}

/// Create an empty store once without replacing another writer's first entry.
pub fn initialize_harness(path: &Path) -> Result<(), HarnessStateError> {
    let (path, _lock) = lock_store(path)?;
    if path.try_exists()? {
        HarnessState::load(&path)?;
    } else {
        write_atomic(&HarnessState::default(), &path)?;
    }
    Ok(())
}

/// Low-level snapshot persistence for setup and tests. Read-modify-write users
/// must use `transact_harness` rather than load + save.
pub(crate) fn save_snapshot(state: &HarnessState, path: &Path) -> Result<(), HarnessStateError> {
    let (path, _lock) = lock_store(path)?;
    write_atomic(state, &path)
}

fn write_atomic(state: &HarnessState, path: &Path) -> Result<(), HarnessStateError> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing harness parent")
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    // NamedTempFile creates private files. Preserve an existing destination's
    // permissions instead of loosening them during replacement.
    match fs::metadata(path) {
        Ok(metadata) => temporary
            .as_file()
            .set_permissions(metadata.permissions())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    serde_json::to_writer_pretty(temporary.as_file_mut(), state)?;
    temporary.as_file_mut().flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_transactions_keep_every_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        let barrier = Arc::new(Barrier::new(12));
        std::thread::scope(|scope| {
            for index in 0..12 {
                let path = &path;
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    transact_harness::<_, HarnessStateError>(path, |state| {
                        state.refinements.push(crate::RefinementEvent {
                            id: index.to_string(),
                            trigger: "test".into(),
                            changes: vec![],
                            evidence: String::new(),
                            outcome: String::new(),
                            created_at: chrono::Utc::now(),
                        });
                        Ok(())
                    })
                    .unwrap();
                });
            }
        });
        let mut ids = HarnessState::load(&path)
            .unwrap()
            .refinements
            .into_iter()
            .map(|event| event.id.parse::<usize>().unwrap())
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, (0..12).collect::<Vec<_>>());
    }

    #[cfg(unix)]
    #[test]
    fn saves_are_private_and_preserve_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = HarnessState::file_path(dir.path());
        initialize_harness(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        HarnessState::default().save_atomic(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[test]
    fn transaction_does_not_replace_corrupt_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("harness_state.json");
        fs::write(&path, "broken").unwrap();
        let result = transact_harness::<_, HarnessStateError>(&path, |_| Ok(()));
        assert!(matches!(result, Err(HarnessStateError::Corrupt(_))));
        assert_eq!(fs::read_to_string(path).unwrap(), "broken");
    }
}
