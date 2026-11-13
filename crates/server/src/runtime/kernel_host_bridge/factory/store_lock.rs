//! Explicit-release guard for the stable Factory store gate.

pub(super) struct StoreLock(std::fs::File);

impl StoreLock {
    pub(super) fn acquire(file: std::fs::File) -> std::io::Result<Self> {
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(Self(file))
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // A fork can retain the same open file description until exec, even
        // with CLOEXEC. Closing only our descriptor would leave its flock held.
        if let Err(error) = fs2::FileExt::unlock(&self.0) {
            tracing::error!(%error, "failed to unlock Factory store before close");
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::StoreLock;

    #[test]
    fn dropping_guard_releases_gate_despite_shared_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.lock");
        let file = std::fs::File::create(&path).unwrap();
        // try_clone shares the same open file description, as fork does.
        let inherited = file.try_clone().unwrap();
        let guard = StoreLock::acquire(file).unwrap();
        let contender = std::fs::File::open(&path).unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&contender).is_err());
        drop(guard);
        fs2::FileExt::try_lock_exclusive(&contender).unwrap();
        fs2::FileExt::unlock(&contender).unwrap();
        drop(inherited);
    }
}
