use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

static HELD_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<std::fs::File>>>> = OnceLock::new();

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
    _file: Arc<std::fs::File>,
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
    pub(crate) fn acquire(rollout_path: &std::path::Path) -> Result<Self, SessionLockContention> {
        use fs2::FileExt;
        use std::io::Seek as _;
        use std::io::Write as _;

        let lock_path = session_lock_path(rollout_path);
        let key = lock_path
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .map(|parent| parent.join(lock_path.file_name().expect("lock file name")))
            .unwrap_or_else(|| lock_path.clone());
        // Retain one locked handle until the last actor in this process releases it.
        // Windows denies reads through a second handle to a locked file, so a
        // pid-based contention bypass cannot work there.
        let mut held = HELD_LOCKS
            .get_or_init(Default::default)
            .lock()
            .expect("session file lock registry mutex should not be poisoned");
        held.retain(|_, file| file.strong_count() != 0);
        if let Some(file) = held.get(&key).and_then(Weak::upgrade) {
            return Ok(Self { _file: file });
        }
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(error) => {
                // Without a lock file we cannot enforce exclusivity; refusing
                // to open would brick sessions on read-only homes, so proceed
                // unlocked (same behavior as before the lock existed).
                tracing::warn!(
                    path = %lock_path.display(),
                    %error,
                    "cannot open session lock file; continuing without cross-process lock"
                );
                return Ok(Self {
                    _file: Arc::new(open_sentinel_file()),
                });
            }
        };
        if let Err(error) = file.try_lock_exclusive() {
            let mut holder_pid = String::new();
            {
                let mut readable = &file;
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
            return Err(SessionLockContention { holder_pid });
        }
        // Record this pid for contention reports; best-effort.
        {
            let mut writable = &file;
            let _ = writable.rewind();
            let _ = writable.set_len(0);
            let _ = writeln!(writable, "{}", std::process::id());
        }
        // LockFileEx prevents other handles from reading the locked bytes.
        // Separate Windows diagnostic metadata is consulted only after an OS
        // lock attempt fails; stale metadata cannot lock a session.
        #[cfg(windows)]
        let _ = std::fs::write(
            lock_path.with_extension("lock.owner"),
            std::process::id().to_string(),
        );
        let file = Arc::new(file);
        held.insert(key, Arc::downgrade(&file));
        Ok(Self { _file: file })
    }
}

/// A lock attempt lost to a live holder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SessionLockContention {
    pub(crate) holder_pid: u32,
}

/// Stand-in file handle for environments where the lock file cannot be
/// created (keeps the struct shape uniform; grants no lock).
fn open_sentinel_file() -> std::fs::File {
    std::fs::OpenOptions::new()
        .read(true)
        .open("/dev/null")
        .unwrap_or_else(|_| {
            // Windows fallback: an invalid handle never matters because the
            // flock itself was already skipped.
            std::fs::File::create(std::env::temp_dir().join(".devo-session-lock-sentinel"))
                .unwrap_or_else(|_| unreachable!("temp dir must be writable"))
        })
}

fn session_lock_path(rollout_path: &std::path::Path) -> std::path::PathBuf {
    let mut name = rollout_path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| std::ffi::OsString::from("session"));
    name.push(".lock");
    rollout_path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;
    use pretty_assertions::assert_eq;

    #[test]
    fn session_lock_is_retained_until_last_same_process_owner_drops() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rollout = dir.path().join("ses_locktest.jsonl");
        std::fs::write(&rollout, "").expect("rollout file");
        let first = SessionFileLock::acquire(&rollout).expect("first acquire");
        let second = SessionFileLock::acquire(&rollout).expect("same-process acquire");
        assert!(Arc::ptr_eq(&first._file, &second._file));

        let contender = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(session_lock_path(&rollout))
            .expect("contender handle");
        assert!(contender.try_lock_exclusive().is_err());
        drop(first);
        assert!(
            contender.try_lock_exclusive().is_err(),
            "second owner must retain lock"
        );

        let probe = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "runtime::session_file_lock::tests::external_process_cannot_acquire",
            ])
            .env("DEVO_LOCK_PROBE_PATH", &rollout)
            .env("DEVO_LOCK_PROBE_HOLDER", std::process::id().to_string())
            .output()
            .expect("cross-process lock probe");
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stdout)
        );
        drop(second);
        contender
            .try_lock_exclusive()
            .expect("last owner releases lock");
        FileExt::unlock(&contender).expect("unlock contender");
        drop(contender);
        SessionFileLock::acquire(&rollout).expect("re-acquire after drop");
    }

    #[test]
    fn external_process_cannot_acquire() {
        let Some(path) = std::env::var_os("DEVO_LOCK_PROBE_PATH") else {
            return;
        };
        let holder_pid = std::env::var("DEVO_LOCK_PROBE_HOLDER")
            .expect("holder pid")
            .parse()
            .expect("numeric holder pid");
        assert_eq!(
            SessionFileLock::acquire(std::path::Path::new(&path))
                .expect_err("another process holds lock"),
            SessionLockContention { holder_pid },
        );
    }
}
