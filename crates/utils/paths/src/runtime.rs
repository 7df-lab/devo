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

/// Find a Python 3.11+ interpreter for source builds in PATH order.
///
/// An activated environment keeps priority, but an old system interpreter
/// cannot hide a suitable interpreter under a versioned name elsewhere on PATH.
/// Return the selected path itself, not the bare name or its symlink target.
pub fn find_source_python(path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    find_python_with_probe(path_var, |path| {
        std::process::Command::new(path)
            .args(["-I", "-S", "--version"])
            .output()
            .is_ok_and(|output| {
                output.status.success()
                    && (supported_python_version(&output.stdout)
                        || supported_python_version(&output.stderr))
            })
    })
}

fn supported_python_version(output: &[u8]) -> bool {
    let Ok(version) = std::str::from_utf8(output) else {
        return false;
    };
    let Some(version) = version.trim().strip_prefix("Python ") else {
        return false;
    };
    let mut parts = version.split('.');
    let major = parts.next().and_then(|part| part.parse::<u32>().ok());
    let minor = parts.next().and_then(|part| part.parse::<u32>().ok());
    matches!((major, minor), (Some(3), Some(11..)) | (Some(4..), Some(_)))
}

fn find_python_with_probe(
    path_var: Option<&std::ffi::OsStr>,
    mut supported: impl FnMut(&Path) -> bool,
) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var?) {
        // App Execution Alias stubs can open the Microsoft Store instead of
        // starting Python. Skip the alias directory, not normal installations.
        if cfg!(windows)
            && dir
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("WindowsApps"))
        {
            continue;
        }
        for name in [
            "python",
            "python3",
            "python3.14",
            "python3.13",
            "python3.12",
            "python3.11",
        ] {
            let candidate = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if candidate.is_file()
                && let Ok(path) = std::path::absolute(candidate)
                && supported(&path)
            {
                return Some(path);
            }
        }
    }
    None
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
    fn source_python_skips_old_system_python_and_preserves_environment_priority() {
        let system = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let venv = tempfile::tempdir().unwrap();
        let create = |dir: &Path, name: &str| {
            let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            std::fs::write(&path, "").unwrap();
            path
        };
        let old = create(system.path(), "python3");
        let versioned = create(user.path(), "python3.13");
        let activated = create(venv.path(), "python");
        let supported = |path: &Path| path != old;

        let mixed = std::env::join_paths([system.path(), user.path()]).unwrap();
        assert_eq!(
            find_python_with_probe(Some(&mixed), supported),
            Some(versioned)
        );
        let mixed = std::env::join_paths([venv.path(), system.path(), user.path()]).unwrap();
        assert_eq!(
            find_python_with_probe(Some(&mixed), supported),
            Some(activated)
        );
        let system_only = std::env::join_paths([system.path()]).unwrap();
        assert_eq!(find_python_with_probe(Some(&system_only), supported), None);
        assert_eq!(find_source_python(None), None);
    }

    #[test]
    fn python_version_must_meet_the_kernel_minimum() {
        for (version, expected) in [
            (b"Python 3.9.6\n".as_slice(), false),
            (b"Python 3.10.19", false),
            (b"Python 3.11.0", true),
            (b"Python 3.13.5", true),
            (b"Python 3.invalid.13", false),
            (b"command unavailable", false),
        ] {
            assert_eq!(supported_python_version(version), expected, "{version:?}");
        }
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
