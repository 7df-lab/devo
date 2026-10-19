//! Locate private release runtimes beside Devo or in an installer-managed cache.
//!
//! A manifest marks an installed bundle. Once present, missing components must
//! fail visibly rather than accidentally using unrelated system runtimes.

use std::path::{Path, PathBuf};

/// Private runtimes shipped in archives or referenced in the installer's cache.
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
    /// Online installations reference immutable cached runtime roots using UTF-8
    /// `runtime/{node,python}.path` files. Offline bundles need no references.
    pub fn from_executable(executable: &Path) -> Option<Self> {
        let parent = executable.parent()?;
        let root = [Some(parent), parent.parent()]
            .into_iter()
            .flatten()
            .find(|root| root.join("runtime/manifest.json").is_file())?
            .to_path_buf();
        Some(Self {
            node: runtime_root(&root, "node").join(if cfg!(windows) {
                "node.exe"
            } else {
                "bin/node"
            }),
            tui: root.join("tui/src/index.js"),
            python: runtime_root(&root, "python").join(if cfg!(windows) {
                "python.exe"
            } else {
                "bin/python3"
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

fn runtime_root(root: &Path, kind: &str) -> PathBuf {
    let reference = root.join(format!("runtime/{kind}.path"));
    if let Err(error) = std::fs::symlink_metadata(&reference)
        && error.kind() == std::io::ErrorKind::NotFound
    {
        return root.join("runtime").join(kind);
    }
    if let Ok(value) = std::fs::read_to_string(reference) {
        let path = PathBuf::from(value.trim_end_matches(['\r', '\n']));
        if path.is_absolute() {
            return path;
        }
    }
    // A broken reference must fail instead of selecting a system interpreter
    // or an obsolete runtime left behind by an interrupted installation.
    root.join(format!("runtime/invalid-{kind}-reference"))
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

    #[test]
    fn cached_runtimes_are_separate_from_app_specific_python_modules() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("application with spaces");
        let node = temp.path().join("cache/node digest/runtime/node");
        let python = temp.path().join("cache/python digest/runtime/python");
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        std::fs::write(root.join("runtime/manifest.json"), "{}").unwrap();
        for (kind, path) in [("node", &node), ("python", &python)] {
            std::fs::write(
                root.join(format!("runtime/{kind}.path")),
                format!("{}\r\n", path.display()),
            )
            .unwrap();
        }
        assert_eq!(
            RuntimeBundle::from_executable(&root.join("devo")),
            Some(RuntimeBundle {
                root: root.clone(),
                node: node.join(if cfg!(windows) {
                    "node.exe"
                } else {
                    "bin/node"
                }),
                python: python.join(if cfg!(windows) {
                    "python.exe"
                } else {
                    "bin/python3"
                }),
                tui: root.join("tui/src/index.js"),
                python_site: root.join("runtime/python-site"),
            })
        );
        std::fs::write(root.join("runtime/node.path"), "relative/path").unwrap();
        assert_eq!(
            runtime_root(&root, "node"),
            root.join("runtime/invalid-node-reference")
        );
    }
}
