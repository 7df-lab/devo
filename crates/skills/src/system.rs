//! Embedded system skill installation.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;

use include_dir::Dir;
use thiserror::Error;

const SYSTEM_SKILLS_DIR: Dir<'_> = include_dir::include_dir!("$DEVO_SKILLS_SAMPLES_DIR");
const SYSTEM_SKILLS_DIR_NAME: &str = ".system";
const SKILLS_DIR_NAME: &str = "skills";
const SYSTEM_SKILLS_MARKER_FILENAME: &str = ".devo-system-skills.marker";
const SYSTEM_SKILLS_MARKER_SALT: &str = "v1";

pub fn system_cache_root_dir(devo_home: &Path) -> PathBuf {
    devo_home.join(SKILLS_DIR_NAME).join(SYSTEM_SKILLS_DIR_NAME)
}

pub fn install_system_skills(devo_home: &Path) -> Result<(), SystemSkillsError> {
    let skills_root_dir = devo_home.join(SKILLS_DIR_NAME);
    fs::create_dir_all(&skills_root_dir)
        .map_err(|source| SystemSkillsError::io("create skills root dir", source))?;

    let dest_system = system_cache_root_dir(devo_home);
    let marker_path = dest_system.join(SYSTEM_SKILLS_MARKER_FILENAME);
    let expected_fingerprint = embedded_system_skills_fingerprint();
    if dest_system.is_dir()
        && read_marker(&marker_path).is_ok_and(|marker| marker == expected_fingerprint)
    {
        return Ok(());
    }

    if dest_system.exists() {
        fs::remove_dir_all(&dest_system)
            .map_err(|source| SystemSkillsError::io("remove existing system skills dir", source))?;
    }

    write_embedded_dir(&SYSTEM_SKILLS_DIR, &dest_system)?;
    fs::write(&marker_path, format!("{expected_fingerprint}\n"))
        .map_err(|source| SystemSkillsError::io("write system skills marker", source))?;
    Ok(())
}

pub fn uninstall_system_skills(devo_home: &Path) {
    let _ = fs::remove_dir_all(system_cache_root_dir(devo_home));
}

fn read_marker(path: &Path) -> Result<String, SystemSkillsError> {
    let mut marker = fs::read_to_string(path)
        .map_err(|source| SystemSkillsError::io("read system skills marker", source))?;
    let start = marker.len() - marker.trim_start().len();
    let end = marker.trim_end().len();
    if start >= end {
        marker.clear();
        return Ok(marker);
    }
    marker.truncate(end);
    if start > 0 {
        marker.drain(..start);
    }
    Ok(marker)
}

fn embedded_system_skills_fingerprint() -> String {
    let mut items = Vec::new();
    collect_fingerprint_items(&SYSTEM_SKILLS_DIR, &mut items);
    items.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    let mut hasher = DefaultHasher::new();
    SYSTEM_SKILLS_MARKER_SALT.hash(&mut hasher);
    for (path, contents_hash) in items {
        path.hash(&mut hasher);
        contents_hash.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

fn collect_fingerprint_items(dir: &Dir<'_>, items: &mut Vec<(String, Option<u64>)>) {
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(subdir) => {
                items.push((subdir.path().to_string_lossy().into_owned(), None));
                collect_fingerprint_items(subdir, items);
            }
            include_dir::DirEntry::File(file) => {
                let mut file_hasher = DefaultHasher::new();
                file.contents().hash(&mut file_hasher);
                items.push((
                    file.path().to_string_lossy().into_owned(),
                    Some(file_hasher.finish()),
                ));
            }
        }
    }
}

fn write_embedded_dir(dir: &Dir<'_>, dest: &Path) -> Result<(), SystemSkillsError> {
    fs::create_dir_all(dest)
        .map_err(|source| SystemSkillsError::io("create system skills dir", source))?;

    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(subdir) => {
                let subdir_dest = dest.join(subdir.path());
                fs::create_dir_all(&subdir_dest).map_err(|source| {
                    SystemSkillsError::io("create system skills subdir", source)
                })?;
                // `include_dir` paths remain relative to the embedded root at
                // every recursion level, so keep `dest` as the cache root.
                write_embedded_dir(subdir, dest)?;
            }
            include_dir::DirEntry::File(file) => {
                let path = dest.join(file.path());
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|source| {
                        SystemSkillsError::io("create system skills file parent", source)
                    })?;
                }
                fs::write(&path, file.contents())
                    .map_err(|source| SystemSkillsError::io("write system skill file", source))?;
            }
        }
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum SystemSkillsError {
    #[error("io error while {action}: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl SystemSkillsError {
    fn io(action: &'static str, source: std::io::Error) -> Self {
        Self::Io { action, source }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pretty_assertions::assert_eq;
    use tempfile::NamedTempFile;

    use super::SYSTEM_SKILLS_DIR;
    use super::collect_fingerprint_items;
    use super::read_marker;

    #[test]
    fn fingerprint_traverses_nested_entries() {
        let mut items = Vec::new();
        collect_fingerprint_items(&SYSTEM_SKILLS_DIR, &mut items);
        let mut paths: Vec<String> = items.into_iter().map(|(path, _)| path).collect();
        paths.sort_unstable();

        assert!(
            paths
                .binary_search(&"skill-creator/SKILL.md".to_string())
                .is_ok()
        );
        assert!(
            paths
                .binary_search(&"skill-creator/scripts/init_skill.py".to_string())
                .is_ok()
        );
        assert_eq!(paths.is_empty(), false);
    }

    #[test]
    fn read_marker_trims_surrounding_whitespace() {
        let file = NamedTempFile::new().expect("marker file");
        fs::write(file.path(), " \nabc\t\n ").expect("write marker");

        assert_eq!(read_marker(file.path()).expect("marker"), "abc");
    }

    #[test]
    fn read_marker_whitespace_only_is_empty() {
        let file = NamedTempFile::new().expect("marker file");
        fs::write(file.path(), " \n\t ").expect("write marker");

        assert_eq!(read_marker(file.path()).expect("marker"), "");
    }
}
