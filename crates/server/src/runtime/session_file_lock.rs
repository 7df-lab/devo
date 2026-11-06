use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::journal_path::canonical_journal_key;

struct HeldLock {
    file: Arc<std::fs::File>,
    owners: usize,
}

static HELD_LOCKS: OnceLock<Mutex<HashMap<PathBuf, HeldLock>>> = OnceLock::new();

/// Exclusive cross-process lock on a session rollout.
///
/// Two Devo processes must never run turns against the same session
/// concurrently: their item sequence numbers and turn records interleave into
/// one corrupt history (both processes append user/assistant items under
/// overlapping ids). The lock is an flock on `<rollout>.jsonl.lock`, held by
/// the owning session actor (`SessionActorState::session_file_lock`); dropping
/// the last owning actor (LRU eviction or process exit) releases it. Crash-safe: the OS
/// releases the flock when the holding process dies, so a crashed Devo never
/// leaves a stale lock behind.
pub(crate) struct SessionFileLock {
    file: Option<Arc<std::fs::File>>,
    /// Canonical lock-file key in [`HELD_LOCKS`].
    key: PathBuf,
}

impl Drop for SessionFileLock {
    fn drop(&mut self) {
        let Some(file) = self.file.take() else {
            return;
        };
        // Keep the registry entry strong while any owner remains and close the
        // final File handle while holding the registry mutex. A concurrent
        // acquire therefore either joins the live entry or opens only after
        // the OS lock is released; it can never observe a half-destroyed entry.
        let mut held = HELD_LOCKS
            .get_or_init(Default::default)
            .lock()
            .expect("session file lock registry mutex should not be poisoned");
        match held.get_mut(&self.key) {
            Some(entry) if entry.owners > 1 => {
                entry.owners -= 1;
                drop(file);
            }
            Some(entry) if entry.owners == 1 => {
                // flock is tied to the open file description, which fork
                // shares. Explicitly unlock before closing so an inherited
                // child descriptor cannot keep the journal locked.
                if let Err(error) = fs2::FileExt::unlock(entry.file.as_ref()) {
                    tracing::error!(
                        path = %self.key.display(),
                        %error,
                        "failed to unlock session journal before close"
                    );
                }
                let entry = held
                    .remove(&self.key)
                    .expect("held lock entry disappeared during drop");
                // Close both the registry handle and this guard's handle while
                // still holding the registry mutex. The next acquire cannot
                // open a second File before this close completes.
                drop(entry.file);
                drop(file);
            }
            Some(_) => {
                tracing::error!("session lock owner count underflow during drop");
                drop(file);
            }
            None => {
                tracing::error!("session lock entry missing during drop");
                drop(file);
            }
        }
    }
}

impl std::fmt::Debug for SessionFileLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionFileLock")
    }
}

impl SessionFileLock {
    /// Acquire the exclusive lock for `rollout_path`. On contention, the
    /// current holder's pid (written into the lock file) is returned so the
    /// refusal can name the conflicting process.
    pub(crate) fn acquire(rollout_path: &std::path::Path) -> Result<Self, SessionLockError> {
        let canonical =
            canonical_journal_key(rollout_path).map_err(|source| SessionLockError::Io {
                path: rollout_path.to_path_buf(),
                source,
            })?;
        let lock_path = session_lock_path(&canonical);
        let key = lock_path.clone();
        // Retain one locked handle until the last actor in this process releases it.
        // Windows denies reads through a second handle to a locked file, so a
        // pid-based contention bypass cannot work there.
        let mut held = HELD_LOCKS
            .get_or_init(Default::default)
            .lock()
            .expect("session file lock registry mutex should not be poisoned");
        if let Some(entry) = held.get_mut(&key) {
            entry.owners += 1;
            return Ok(Self {
                file: Some(Arc::clone(&entry.file)),
                key,
            });
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| SessionLockError::Io {
                path: lock_path.clone(),
                source,
            })?;
        lock_exclusive(&file, &lock_path)?;
        let file = Arc::new(file);
        held.insert(
            key.clone(),
            HeldLock {
                file: Arc::clone(&file),
                owners: 1,
            },
        );
        Ok(Self {
            file: Some(file),
            key,
        })
    }
}

fn lock_exclusive(
    file: &std::fs::File,
    lock_path: &std::path::Path,
) -> Result<(), SessionLockError> {
    use fs2::FileExt;
    use std::io::Seek as _;
    use std::io::Write as _;

    if let Err(error) = file.try_lock_exclusive() {
        let contended = error.kind() == std::io::ErrorKind::WouldBlock
            || error.raw_os_error() == fs2::lock_contended_error().raw_os_error();
        if !contended {
            return Err(SessionLockError::Io {
                path: lock_path.to_path_buf(),
                source: error,
            });
        }
        let mut holder_pid = String::new();
        {
            let mut readable = file;
            let _ = readable.rewind();
            let _ = std::io::Read::read_to_string(&mut readable, &mut holder_pid);
        }
        let holder_pid = holder_pid.trim().parse::<u32>().unwrap_or(0);
        #[cfg(windows)]
        let holder_pid = std::fs::read_to_string(lock_path.with_extension("lock.owner"))
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(holder_pid);
        tracing::warn!(
            path = %lock_path.display(),
            %error,
            holder_pid,
            "session rollout is locked by another Devo process"
        );
        return Err(SessionLockError::Contention { holder_pid });
    }
    {
        let mut writable = file;
        let _ = writable.rewind();
        let _ = writable.set_len(0);
        let _ = writeln!(writable, "{}", std::process::id());
    }
    #[cfg(windows)]
    let _ = std::fs::write(
        lock_path.with_extension("lock.owner"),
        std::process::id().to_string(),
    );
    Ok(())
}

/// A lock attempt failed either because another process holds it or because
/// the lock file itself could not be resolved/opened.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SessionLockError {
    #[error("session rollout is locked by another Devo process (pid {holder_pid})")]
    Contention { holder_pid: u32 },
    #[error("session lock I/O error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn session_lock_path(canonical_journal: &std::path::Path) -> std::path::PathBuf {
    let mut name = canonical_journal
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| std::ffi::OsString::from("session"));
    name.push(".lock");
    canonical_journal.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[cfg(unix)]
    struct InheritedFdHelper {
        child: Option<std::process::Child>,
        sync: Option<std::os::unix::net::UnixStream>,
    }

    #[cfg(unix)]
    impl Drop for InheritedFdHelper {
        fn drop(&mut self) {
            use std::io::Write as _;
            if let Some(mut sync) = self.sync.take() {
                let _ = sync.write_all(&[2]);
            }
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    #[test]
    fn session_lock_is_retained_until_last_same_process_owner_drops() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rollout = dir.path().join("ses_locktest.jsonl");
        std::fs::write(&rollout, "").expect("rollout file");
        let first = SessionFileLock::acquire(&rollout).expect("first acquire");
        let second = SessionFileLock::acquire(&rollout).expect("same-process acquire");
        assert!(Arc::ptr_eq(
            first.file.as_ref().expect("first file"),
            second.file.as_ref().expect("second file")
        ));

        run_lock_probe(&rollout, /*expect_free*/ false);
        drop(first);
        run_lock_probe(&rollout, /*expect_free*/ false);
        drop(second);
        run_lock_probe(&rollout, /*expect_free*/ true);
        SessionFileLock::acquire(&rollout).expect("re-acquire after drop");
    }

    fn run_lock_probe(rollout: &std::path::Path, expect_free: bool) {
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "runtime::session_file_lock::tests::external_process_cannot_acquire",
            ])
            .env("DEVO_LOCK_PROBE_PATH", rollout)
            .env("DEVO_LOCK_PROBE_HOLDER", std::process::id().to_string())
            .env(
                "DEVO_LOCK_PROBE_EXPECT",
                if expect_free { "free" } else { "held" },
            );
        let probe = command.output().expect("cross-process lock probe");
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stdout)
        );
    }

    #[test]
    fn registry_entry_and_fd_are_released_after_last_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rollout = dir.path().join("ses_registry_release.jsonl");
        std::fs::write(&rollout, "").expect("rollout file");
        let key = session_lock_path(&canonical_journal_key(&rollout).expect("canonical journal"));
        {
            let guard = SessionFileLock::acquire(&rollout).expect("acquire");
            {
                let held = HELD_LOCKS
                    .get_or_init(Default::default)
                    .lock()
                    .expect("registry");
                assert_eq!(held.get(&key).map(|entry| entry.owners), Some(1));
            }
            drop(guard);
        }
        let held = HELD_LOCKS
            .get_or_init(Default::default)
            .lock()
            .expect("registry");
        assert!(
            !held.contains_key(&key),
            "last drop must remove registry entry and close its fd"
        );
    }

    #[test]
    fn concurrent_acquire_drop_same_journal_never_reports_contention() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rollout = dir
            .path()
            .join(format!("ses_acquire_drop_{}.jsonl", uuid::Uuid::new_v4()));
        std::fs::write(&rollout, "").expect("rollout file");
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let rollout = rollout.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..200 {
                        let guard = SessionFileLock::acquire(&rollout)
                            .expect("same-process acquire must not report contention");
                        drop(guard);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("acquire/drop worker");
        }
    }

    #[cfg(unix)]
    #[test]
    fn last_drop_unlock_releases_lock_held_by_inherited_descriptor() {
        use std::io::Read as _;
        use std::io::Write as _;
        use std::os::unix::io::AsRawFd;
        use std::os::unix::net::UnixStream;

        let dir = tempfile::tempdir().expect("tempdir");
        let rollout = dir.path().join("ses_inherited_lock.jsonl");
        std::fs::write(&rollout, "").expect("rollout file");
        let guard = SessionFileLock::acquire(&rollout).expect("acquire");
        let lock_fd = guard.file.as_ref().expect("lock file").as_raw_fd();
        let (parent_sync, child_sync) = UnixStream::pair().expect("sync pair");
        let sync_fd = child_sync.as_raw_fd();
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "runtime::session_file_lock::tests::inherited_lock_descriptor_helper",
            ])
            .env("DEVO_INHERITED_LOCK_FD", lock_fd.to_string())
            .env("DEVO_INHERITED_SYNC_FD", sync_fd.to_string());
        // Only the child clears CLOEXEC, in pre_exec. Parent fds stay CLOEXEC;
        // unrelated parallel spawns never see these descriptors.
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(move || {
                for fd in [lock_fd, sync_fd] {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let child = command.spawn().expect("spawn inherited-fd helper");
        drop(child_sync);
        let mut helper = InheritedFdHelper {
            child: Some(child),
            sync: Some(parent_sync),
        };
        let mut ready = [0u8; 1];
        helper
            .sync
            .as_mut()
            .expect("parent sync")
            .read_exact(&mut ready)
            .expect("child inherited-fd ready");
        assert_eq!(ready, [1]);

        // The child still owns an inherited descriptor for the locked OFD.
        // Last-owner Drop must unlock the shared OFD before closing.
        drop(guard);
        let reacquired = SessionFileLock::acquire(&rollout)
            .expect("parent unlock must release the shared OFD lock");
        drop(reacquired);

        helper
            .sync
            .as_mut()
            .expect("parent sync")
            .write_all(&[2])
            .expect("release helper");
        let status = helper
            .child
            .as_mut()
            .expect("child")
            .wait()
            .expect("inherited-fd helper joins");
        helper.child = None;
        assert!(status.success(), "inherited-fd helper failed: {status}");
    }

    #[cfg(unix)]
    #[test]
    fn inherited_lock_descriptor_helper() {
        use std::io::Read as _;
        use std::io::Write as _;
        use std::os::unix::io::FromRawFd;

        let Some(lock_fd) = std::env::var("DEVO_INHERITED_LOCK_FD")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
        else {
            return;
        };
        let sync_fd = std::env::var("DEVO_INHERITED_SYNC_FD")
            .expect("sync fd")
            .parse::<i32>()
            .expect("numeric sync fd");
        assert!(
            unsafe { libc::fcntl(lock_fd, libc::F_GETFD) } >= 0,
            "inherited lock fd must still be open"
        );
        let mut sync = unsafe { std::os::unix::net::UnixStream::from_raw_fd(sync_fd) };
        sync.write_all(&[1]).expect("helper ready");
        let mut release = [0u8; 1];
        sync.read_exact(&mut release).expect("helper release");
        assert_eq!(release, [2]);
        // Keep the inherited lock fd open until process exit.
    }

    #[test]
    fn external_process_cannot_acquire() {
        let Some(path) = std::env::var_os("DEVO_LOCK_PROBE_PATH") else {
            return;
        };
        let expect = std::env::var("DEVO_LOCK_PROBE_EXPECT").unwrap_or_else(|_| "held".into());
        if expect == "free" {
            SessionFileLock::acquire(std::path::Path::new(&path))
                .expect("probe expected a free lock");
            return;
        }
        let holder_pid: u32 = std::env::var("DEVO_LOCK_PROBE_HOLDER")
            .expect("holder pid")
            .parse()
            .expect("numeric holder pid");
        match SessionFileLock::acquire(std::path::Path::new(&path))
            .expect_err("another process holds lock")
        {
            SessionLockError::Contention { holder_pid: actual } => assert_eq!(actual, holder_pid),
            other => panic!("expected contention, got {other:?}"),
        }
    }
}
