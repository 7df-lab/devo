//! Locate private release runtimes relative to the running Devo executable.
//!
//! A manifest marks an installed bundle. Once present, missing components must
//! fail visibly rather than accidentally using unrelated system runtimes.

use std::path::{Path, PathBuf};

/// The relocatable runtime shipped with CLI archives and Desktop packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBundle {
    pub root: PathBuf,
    pub node: PathBuf,
    pub tui: PathBuf,
    pub python: PathBuf,
    pub python_site: PathBuf,
}

impl RuntimeBundle {
    /// Discover a bundle beside an executable, including the older `bin/` layout.
    pub fn from_executable(executable: &Path) -> Option<Self> {
        let parent = executable.parent()?;
        let root = [Some(parent), parent.parent()]
            .into_iter()
            .flatten()
            .find(|root| root.join("runtime/manifest.json").is_file())?
            .to_path_buf();
        Some(Self {
            node: root.join(if cfg!(windows) {
                "runtime/node/node.exe"
            } else {
                "runtime/node/bin/node"
            }),
            tui: root.join("tui/src/index.js"),
            python: root.join(if cfg!(windows) {
                "runtime/python/python.exe"
            } else {
                "runtime/python/bin/python3"
            }),
            python_site: root.join("runtime/python-site"),
            root,
        })
    }

    /// Discover the running application's private runtime, if it is installed.
    pub fn current() -> Option<Self> {
        Self::from_executable(&std::env::current_exe().ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn source_binary_has_no_bundle() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            RuntimeBundle::from_executable(&temp.path().join("devo")),
            None
        );
    }

    #[test]
    fn bundle_is_relocatable_and_broken_components_do_not_fall_back() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join("runtime")).unwrap();
        std::fs::write(root.join("runtime/manifest.json"), "{}").unwrap();
        let expected = Some(RuntimeBundle {
            root: root.to_path_buf(),
            node: root.join(if cfg!(windows) {
                "runtime/node/node.exe"
            } else {
                "runtime/node/bin/node"
            }),
            tui: root.join("tui/src/index.js"),
            python: root.join(if cfg!(windows) {
                "runtime/python/python.exe"
            } else {
                "runtime/python/bin/python3"
            }),
            python_site: root.join("runtime/python-site"),
        });
        assert_eq!(RuntimeBundle::from_executable(&root.join("devo")), expected);
        assert_eq!(
            RuntimeBundle::from_executable(&root.join("bin/devo")),
            expected
        );
    }
}
