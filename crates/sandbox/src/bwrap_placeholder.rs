//! Fail-closed read-deny placeholders for Linux bwrap bind-over.
//!
//! Binding a zero-permission placeholder over a deny path makes it unreadable
//! (EACCES/EPERM) and unwritable (read-only mount) inside the sandbox. Each
//! bwrap launch creates at most one private mode-0700 directory (via
//! `mkdtemp`) under `devo_home` and creates its placeholders exclusively
//! inside it, so a writable home directory cannot swap them for symlinks
//! between creation and bind.
//!
//! Cleanup lives in [`crate::wrap`]: the spawner removes the directory once
//! the launch's mounts are up, and a janitor removes stale directories left
//! behind by crashed processes.

#[cfg(feature = "enforce")]
use anyhow::Context;
use std::path::{Path, PathBuf};

#[cfg(feature = "enforce")]
use crate::deny;
#[cfg(feature = "enforce")]
use crate::wrap::PLACEHOLDER_DIR_PREFIX;

/// Choose file vs directory placeholder for a deny path (existing dirs need a
/// dir bind) and create it inside `directory` under an index-unique name.
#[cfg(feature = "enforce")]
pub(crate) fn bwrap_blocked_source_in(
    directory: &Path,
    path: &Path,
    index: usize,
) -> anyhow::Result<PathBuf> {
    let kind = if deny::deny_path_is_dir(path) {
        BwrapPlaceholderKind::Directory
    } else {
        BwrapPlaceholderKind::File
    };
    bwrap_blocked_placeholder_in(directory, kind, &format!("{}-{index}", kind.name()))
}

/// Without kernel enforcement there are no read-deny placeholders to bind over.
#[cfg(not(feature = "enforce"))]
pub(crate) fn bwrap_blocked_source_in(
    _directory: &Path,
    _path: &Path,
    _index: usize,
) -> anyhow::Result<PathBuf> {
    anyhow::bail!("bwrap read-deny placeholders require the 'enforce' feature")
}

/// chmod a placeholder to mode 000 so a bwrap bind-over yields EPERM on read.
#[cfg(feature = "enforce")]
fn chmod_000(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .with_context(|| format!("could not inspect bwrap placeholder {}", path.display()))?
        .permissions();
    permissions.set_mode(0o000);
    std::fs::set_permissions(path, permissions)
        .with_context(|| format!("could not secure bwrap placeholder {}", path.display()))?;
    Ok(())
}

#[cfg(feature = "enforce")]
#[derive(Clone, Copy)]
enum BwrapPlaceholderKind {
    File,
    Directory,
}

#[cfg(feature = "enforce")]
impl BwrapPlaceholderKind {
    fn name(self) -> &'static str {
        match self {
            Self::File => "sandbox-blocked",
            Self::Directory => "sandbox-blocked-dir",
        }
    }

    fn is_expected_type(self, file_type: std::fs::FileType) -> bool {
        match self {
            Self::File => file_type.is_file(),
            Self::Directory => file_type.is_dir(),
        }
    }
}

/// Zero-permission placeholder directory for one bwrap launch, in a private
/// random directory under `devo_home`.
///
/// The old PID-named files were predictable entries in a writable home and
/// could be replaced with symlinks. Each bwrap launch now creates a mode-0700
/// directory with `mkdtemp`, then creates the placeholder exclusively inside it.
#[cfg(feature = "enforce")]
pub(crate) fn create_private_bwrap_placeholder_directory() -> anyhow::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;

    let devo_home =
        crate::paths::devo_home().context("could not resolve bwrap placeholder root")?;
    match std::fs::symlink_metadata(&devo_home) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!(
            "bwrap placeholder root {} is not a real directory",
            devo_home.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&devo_home).with_context(|| {
                format!(
                    "could not create bwrap placeholder root {}",
                    devo_home.display()
                )
            })?;
            let metadata = std::fs::symlink_metadata(&devo_home).with_context(|| {
                format!(
                    "could not inspect bwrap placeholder root {}",
                    devo_home.display()
                )
            })?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                anyhow::bail!(
                    "bwrap placeholder root {} is not a real directory",
                    devo_home.display()
                );
            }
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "could not inspect bwrap placeholder root {}",
                    devo_home.display()
                )
            });
        }
    }

    let template = devo_home.join(format!("{PLACEHOLDER_DIR_PREFIX}XXXXXX"));
    let mut template_bytes = template.into_os_string().into_vec();
    if template_bytes.contains(&0) {
        anyhow::bail!("bwrap placeholder template contains an interior NUL");
    }
    template_bytes.push(0);

    // SAFETY: `template_bytes` is mutable, NUL-terminated, and ends in six Xs.
    if unsafe { libc::mkdtemp(template_bytes.as_mut_ptr().cast()) }.is_null() {
        return Err(std::io::Error::last_os_error())
            .context("could not create private bwrap placeholder directory");
    }
    template_bytes.pop();
    let directory = PathBuf::from(std::ffi::OsString::from_vec(template_bytes));

    let mut permissions = std::fs::metadata(&directory)
        .with_context(|| {
            format!(
                "could not inspect private bwrap placeholder directory {}",
                directory.display()
            )
        })?
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&directory, permissions).with_context(|| {
        format!(
            "could not secure private bwrap placeholder directory {}",
            directory.display()
        )
    })?;
    if !is_private_bwrap_placeholder_directory(&directory) {
        anyhow::bail!(
            "bwrap placeholder directory {} failed validation",
            directory.display()
        );
    }

    Ok(directory)
}

/// Without kernel enforcement no placeholder directory is ever needed; refuse
/// rather than creating an unsecured one.
#[cfg(not(feature = "enforce"))]
pub(crate) fn create_private_bwrap_placeholder_directory() -> anyhow::Result<PathBuf> {
    anyhow::bail!("bwrap read-deny placeholders require the 'enforce' feature")
}

#[cfg(feature = "enforce")]
fn is_private_bwrap_placeholder_directory(directory: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = std::fs::symlink_metadata(directory) else {
        return false;
    };
    metadata.file_type().is_dir()
        && !metadata.file_type().is_symlink()
        && metadata.permissions().mode() & 0o077 == 0
}

#[cfg(feature = "enforce")]
fn bwrap_blocked_placeholder_in(
    directory: &Path,
    kind: BwrapPlaceholderKind,
    name: &str,
) -> anyhow::Result<PathBuf> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    if !is_private_bwrap_placeholder_directory(directory) {
        anyhow::bail!(
            "bwrap placeholder directory {} is not private",
            directory.display()
        );
    }

    let path = directory.join(name);
    match kind {
        BwrapPlaceholderKind::File => {
            OpenOptions::new()
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .write(true)
                .open(&path)
                .with_context(|| {
                    format!("could not create bwrap placeholder {}", path.display())
                })?;
        }
        BwrapPlaceholderKind::Directory => {
            std::fs::create_dir(&path).with_context(|| {
                format!("could not create bwrap placeholder {}", path.display())
            })?;
        }
    }
    chmod_000(&path)?;
    if !is_valid_bwrap_placeholder(&path, kind) {
        anyhow::bail!("bwrap placeholder {} failed validation", path.display());
    }
    Ok(path)
}

#[cfg(feature = "enforce")]
fn is_valid_bwrap_placeholder(path: &Path, kind: BwrapPlaceholderKind) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    !metadata.file_type().is_symlink()
        && kind.is_expected_type(metadata.file_type())
        && metadata.permissions().mode() & 0o777 == 0
}

#[cfg(all(test, feature = "enforce"))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn private_placeholder_test_directory(tag: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "devo-bwrap-placeholder-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("create placeholder test directory");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("make placeholder test directory private");
        directory
    }

    #[test]
    fn bwrap_placeholder_is_exclusive_and_validated() {
        let directory = private_placeholder_test_directory("valid");
        let placeholder =
            bwrap_blocked_placeholder_in(&directory, BwrapPlaceholderKind::File, "sandbox-blocked")
                .expect("create placeholder exclusively");
        assert!(is_valid_bwrap_placeholder(
            &placeholder,
            BwrapPlaceholderKind::File
        ));
        assert!(
            bwrap_blocked_placeholder_in(&directory, BwrapPlaceholderKind::File, "sandbox-blocked")
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn bwrap_placeholder_rejects_a_preexisting_symlink() {
        use std::os::unix::fs::symlink;

        let directory = private_placeholder_test_directory("symlink");
        let target = directory.join("outside-target");
        std::fs::write(&target, "must remain unchanged").expect("write symlink target");
        let placeholder = directory.join(BwrapPlaceholderKind::File.name());
        symlink(&target, &placeholder).expect("create preexisting symlink");
        assert!(
            bwrap_blocked_placeholder_in(&directory, BwrapPlaceholderKind::File, "sandbox-blocked")
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("read symlink target"),
            "must remain unchanged"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn bwrap_blocked_source_picks_kind_and_unique_names() {
        let directory = private_placeholder_test_directory("kinds");
        let denied_dir = directory.join("denied-dir");
        std::fs::create_dir(&denied_dir).expect("create denied directory");
        let denied_file = directory.join("denied-file");
        std::fs::write(&denied_file, "secret").expect("write denied file");

        let dir_source = bwrap_blocked_source_in(&directory, &denied_dir, 0)
            .expect("directory placeholder source");
        let file_source =
            bwrap_blocked_source_in(&directory, &denied_file, 1).expect("file placeholder source");

        assert_ne!(dir_source, file_source);
        assert!(dir_source.ends_with("sandbox-blocked-dir-0"));
        assert!(file_source.ends_with("sandbox-blocked-1"));
        assert!(is_valid_bwrap_placeholder(
            &dir_source,
            BwrapPlaceholderKind::Directory
        ));
        assert!(is_valid_bwrap_placeholder(
            &file_source,
            BwrapPlaceholderKind::File
        ));
        let _ = std::fs::remove_dir_all(&directory);
    }
}
