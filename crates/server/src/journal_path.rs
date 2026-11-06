//! Canonical physical identity for session journals.
//!
//! Locks, in-process append state, event watermarks, and unlink guards must all
//! agree on one physical file. Canonicalizing the existing file resolves
//! symlink aliases; for a not-yet-created file we canonicalize the parent and
//! join the file name. There is deliberately no lexical fallback: a failure to
//! resolve ownership is a write error, not a second lock identity.

use std::io;
use std::path::Path;
use std::path::PathBuf;

pub(crate) fn canonical_journal_key(rollout_path: &Path) -> io::Result<PathBuf> {
    match std::fs::canonicalize(rollout_path) {
        Ok(path) => return Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = rollout_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("journal path has no parent: {}", rollout_path.display()),
        )
    })?;
    let file_name = rollout_path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("journal path has no file name: {}", rollout_path.display()),
        )
    })?;
    let parent = std::fs::canonicalize(parent)?;
    Ok(parent.join(file_name))
}
