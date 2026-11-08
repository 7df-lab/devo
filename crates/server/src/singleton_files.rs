//! Owner-only singleton files. Daemon startup fails closed on unsupported ACLs.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub(crate) fn daemon_home(home: &Path) -> Result<PathBuf> {
    #[cfg(not(unix))]
    {
        let _ = home;
        bail!(
            "background daemon requires Unix owner-only file protection; Windows ACL support is not implemented"
        )
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(home)?;
        if fs::symlink_metadata(home)?.file_type().is_symlink() {
            bail!("daemon home must not be a symbolic link");
        }
        let canonical = home.canonicalize()?;
        let uid = unsafe { libc::geteuid() };
        for (index, ancestor) in canonical.ancestors().enumerate() {
            let metadata = fs::metadata(ancestor)?;
            let mode = metadata.permissions().mode();
            if (index == 0 && metadata.uid() != uid)
                || (metadata.uid() != uid && metadata.uid() != 0)
                || (mode & 0o022 != 0 && !(index > 0 && metadata.uid() == 0 && mode & 0o1000 != 0))
            {
                bail!(
                    "daemon home has an unsafe owner or writable ancestor: {}",
                    ancestor.display()
                );
            }
        }
        Ok(canonical)
    }
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options
}

fn validate(file: &File, private: bool) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("singleton path is not a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || (private && metadata.permissions().mode() & 0o077 != 0)
        {
            bail!(
                "singleton file must be owned by this user with owner-only permissions; stop the old server before restarting"
            );
        }
    }
    #[cfg(not(unix))]
    let _ = private;
    Ok(())
}

pub(crate) fn open_lock(path: &Path) -> Result<File> {
    let file = options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    validate(&file, /*private*/ false)?;
    Ok(file)
}

pub(crate) fn read_metadata(path: &Path) -> Result<Vec<u8>> {
    let mut file = options().read(true).open(path)?;
    validate(&file, /*private*/ true)?;
    if file.metadata()?.len() > 16 * 1024 {
        bail!("singleton metadata exceeds size limit");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn publish_metadata(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = options().create_new(true).write(true).open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path).context("publish singleton metadata")?;
        #[cfg(unix)]
        File::open(path.parent().context("metadata parent")?)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn metadata_is_private_and_symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        publish_metadata(&path, b"private").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(read_metadata(&path).unwrap(), b"private");
        let link = dir.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(read_metadata(&link).is_err());
        assert!(open_lock(&link).is_err());
    }

    #[test]
    fn daemon_rejects_writable_parent_even_when_home_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(daemon_home(&home).is_err());
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(daemon_home(&home).unwrap(), home);
    }
}
