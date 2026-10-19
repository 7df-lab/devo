//! Filesystem / network permission types for the Windows sandbox protocol bridge.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use crate::path_util::canonicalize_preserving_symlinks;
use devo_util_paths::absolute_path::AbsolutePathBuf;
use devo_util_paths::absolute_path::PathUri;
use globset::GlobBuilder;
use globset::GlobMatcher;
use serde::Deserialize;
use serde::Serialize;
use strum_macros::Display;
use tracing::error;

use super::legacy_protocol::NetworkAccess;
use super::legacy_protocol::SandboxPolicy;
use super::legacy_protocol::WritableRoot;

const PROTECTED_METADATA_GIT_PATH_NAME: &str = ".git";
const PROTECTED_METADATA_AGENTS_PATH_NAME: &str = ".agents";
const PROTECTED_METADATA_DEVO_PATH_NAME: &str = ".devo";
const PROTECTED_METADATA_LEGACY_AGENT_PATH_NAME: &str = ".codex";

const PROTECTED_METADATA_PATH_NAMES: &[&str] = &[
    PROTECTED_METADATA_GIT_PATH_NAME,
    PROTECTED_METADATA_AGENTS_PATH_NAME,
    PROTECTED_METADATA_DEVO_PATH_NAME,
    // Legacy `.codex` agent metadata directory — still protected during migration.
    PROTECTED_METADATA_LEGACY_AGENT_PATH_NAME,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, Default)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum NetworkSandboxPolicy {
    #[default]
    Restricted,
    Enabled,
}

impl NetworkSandboxPolicy {
    pub fn is_enabled(self) -> bool {
        matches!(self, NetworkSandboxPolicy::Enabled)
    }
}

/// Access mode for a filesystem entry.
///
/// When two equally specific entries target the same path, we compare these by
/// conflict precedence rather than by capability breadth: `deny` beats
/// `write`, and `write` beats `read`.
#[derive(
    Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Display,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum FileSystemAccessMode {
    Read,
    Write,
    /// `none` is a legacy input alias retained temporarily for compatibility.
    #[serde(alias = "none")]
    Deny,
}

impl FileSystemAccessMode {
    pub fn can_read(self) -> bool {
        !matches!(self, FileSystemAccessMode::Deny)
    }

    pub fn can_write(self) -> bool {
        matches!(self, FileSystemAccessMode::Write)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileSystemSpecialPath {
    Root,
    Minimal,
    #[serde(alias = "current_working_directory")]
    ProjectRoots {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
    Tmpdir,
    SlashTmp,
    /// WARNING: `:special_path` tokens are part of config compatibility.
    /// Do not make older runtimes reject newly introduced tokens.
    /// New parser support should be additive, while unknown values must stay
    /// representable so config from a newer release degrades to warn-and-ignore
    /// instead of failing to load. Older releases rejected unknown values here,
    /// which broke forward compatibility for newer config.
    /// Preserves future special-path tokens so older runtimes can ignore them
    /// without rejecting config authored by a newer release.
    Unknown {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
}

impl FileSystemSpecialPath {
    pub fn project_roots(subpath: Option<String>) -> Self {
        Self::ProjectRoots { subpath }
    }

    pub fn unknown(path: impl Into<String>, subpath: Option<String>) -> Self {
        Self::Unknown {
            path: path.into(),
            subpath,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileSystemSandboxEntry {
    pub path: FileSystemPath,
    pub access: FileSystemAccessMode,
}

fn sandbox_entry(path: FileSystemPath, access: FileSystemAccessMode) -> FileSystemSandboxEntry {
    FileSystemSandboxEntry { path, access }
}

fn special_entry(
    value: FileSystemSpecialPath,
    access: FileSystemAccessMode,
) -> FileSystemSandboxEntry {
    sandbox_entry(FileSystemPath::Special { value }, access)
}

fn path_entry(path: impl Into<PathUri>, access: FileSystemAccessMode) -> FileSystemSandboxEntry {
    sandbox_entry(FileSystemPath::from_path(path), access)
}

fn workspace_write_core_entries(
    writable_roots: impl IntoIterator<Item = AbsolutePathBuf>,
    exclude_tmpdir_env_var: bool,
    exclude_slash_tmp: bool,
) -> Vec<FileSystemSandboxEntry> {
    let mut entries = vec![
        special_entry(FileSystemSpecialPath::Root, FileSystemAccessMode::Read),
        special_entry(
            FileSystemSpecialPath::project_roots(/*subpath*/ None),
            FileSystemAccessMode::Write,
        ),
    ];
    if !exclude_slash_tmp {
        entries.push(special_entry(
            FileSystemSpecialPath::SlashTmp,
            FileSystemAccessMode::Write,
        ));
    }
    if !exclude_tmpdir_env_var {
        entries.push(special_entry(
            FileSystemSpecialPath::Tmpdir,
            FileSystemAccessMode::Write,
        ));
    }
    entries.extend(
        writable_roots
            .into_iter()
            .map(|path| path_entry(path, FileSystemAccessMode::Write)),
    );
    entries
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, Default)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum FileSystemSandboxKind {
    #[default]
    Restricted,
    Unrestricted,
    ExternalSandbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSystemSandboxPolicy {
    pub kind: FileSystemSandboxKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob_scan_max_depth: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<FileSystemSandboxEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedFileSystemEntry {
    path: AbsolutePathBuf,
    access: FileSystemAccessMode,
}

/// Runtime matcher for read-deny entries in a filesystem sandbox policy.
pub struct ReadDenyMatcher {
    denied_candidates: Vec<Vec<PathBuf>>,
    deny_read_matchers: Vec<GlobMatcher>,
}

impl ReadDenyMatcher {
    /// Builds a matcher from exact deny-read roots and deny-read glob entries.
    ///
    /// Returns `None` when the policy has no deny-read restrictions.
    ///
    /// Malformed glob patterns return an error so callers fail closed instead of
    /// silently broadening the set of paths they mutate before execution starts.
    pub fn try_new(
        file_system_sandbox_policy: &FileSystemSandboxPolicy,
        cwd: &Path,
    ) -> Result<Option<Self>, String> {
        if !file_system_sandbox_policy.has_denied_read_restrictions() {
            return Ok(None);
        }

        // Exact roots are stored as all meaningful path spellings we can derive
        // cheaply. This lets direct tool checks catch both a symlink path and
        // its canonical target without changing the policy entries themselves.
        let denied_candidates = file_system_sandbox_policy
            .get_unreadable_roots_with_cwd(cwd)
            .into_iter()
            .map(|path| normalized_and_canonical_candidates(path.as_path()))
            .collect();
        // Pattern entries stay as policy-level globs. They are matched at read
        // time here instead of being snapshotted to startup filesystem state.
        let mut deny_read_matchers = Vec::new();
        for pattern in file_system_sandbox_policy.get_unreadable_globs_with_cwd(cwd) {
            let matcher = build_glob_matcher(&pattern)
                .map_err(|err| format!("invalid deny-read glob pattern `{pattern}`: {err}"))?;
            deny_read_matchers.push(matcher);
        }
        Ok(Some(Self {
            denied_candidates,
            deny_read_matchers,
        }))
    }

    /// Returns whether `path` is denied by the policy used to build this matcher.
    pub fn is_read_denied(&self, path: &Path) -> bool {
        // Check exact roots against each candidate spelling before evaluating
        // glob matchers. Exact entries are subtree denies; glob entries match
        // according to the pattern compiler's path-separator rules.
        let path_candidates = normalized_and_canonical_candidates(path);
        if self.denied_candidates.iter().any(|denied_candidates| {
            path_candidates.iter().any(|candidate| {
                denied_candidates.iter().any(|denied_candidate| {
                    candidate == denied_candidate || candidate.starts_with(denied_candidate)
                })
            })
        }) {
            return true;
        }

        self.deny_read_matchers.iter().any(|matcher| {
            path_candidates
                .iter()
                .any(|candidate| matcher.is_match(candidate))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileSystemPath {
    Path {
        path: PathUri,
    },
    /// A git-style glob pattern. Pattern entries currently support
    /// FileSystemAccessMode::Deny only.
    GlobPattern {
        pattern: String,
    },
    Special {
        value: FileSystemSpecialPath,
    },
}

impl FileSystemPath {
    pub fn from_path(path: impl Into<PathUri>) -> Self {
        Self::Path { path: path.into() }
    }
}

const PROJECT_ROOTS_GLOB_PATTERN_PREFIX: &str = "devo-project-roots://";

#[allow(dead_code)] // consumed from sibling-module tests (`resolved_permissions`, `permissions`).
pub fn project_roots_glob_pattern(subpath: &Path) -> String {
    format!("{PROJECT_ROOTS_GLOB_PATTERN_PREFIX}{}", subpath.display())
}

fn read_only_file_system_entries() -> Vec<FileSystemSandboxEntry> {
    vec![special_entry(
        FileSystemSpecialPath::Root,
        FileSystemAccessMode::Read,
    )]
}

impl Default for FileSystemSandboxPolicy {
    fn default() -> Self {
        Self::read_only()
    }
}

impl FileSystemSandboxPolicy {
    pub fn read_only() -> Self {
        Self::restricted(read_only_file_system_entries())
    }

    pub fn unrestricted() -> Self {
        Self {
            kind: FileSystemSandboxKind::Unrestricted,
            glob_scan_max_depth: None,
            entries: Vec::new(),
        }
    }

    pub fn external_sandbox() -> Self {
        Self {
            kind: FileSystemSandboxKind::ExternalSandbox,
            glob_scan_max_depth: None,
            entries: Vec::new(),
        }
    }

    pub fn restricted(entries: Vec<FileSystemSandboxEntry>) -> Self {
        Self {
            kind: FileSystemSandboxKind::Restricted,
            glob_scan_max_depth: None,
            entries,
        }
    }

    fn has_root_access(&self, predicate: impl Fn(FileSystemAccessMode) -> bool) -> bool {
        matches!(self.kind, FileSystemSandboxKind::Restricted)
            && self.entries.iter().any(|entry| {
                matches!(
                    &entry.path,
                    FileSystemPath::Special { value }
                        if matches!(value, FileSystemSpecialPath::Root) && predicate(entry.access)
                )
            })
    }

    pub fn has_denied_read_restrictions(&self) -> bool {
        matches!(self.kind, FileSystemSandboxKind::Restricted)
            && self
                .entries
                .iter()
                .any(|entry| entry.access == FileSystemAccessMode::Deny)
    }

    /// Returns true when a restricted policy contains any entry that really
    /// reduces a broader `:root = write` grant.
    ///
    /// Raw entry presence is not enough here: an equally specific `write`
    /// entry for the same target wins under the normal precedence rules, so a
    /// shadowed `read` entry must not downgrade the policy out of full-disk
    /// write mode.
    fn has_write_narrowing_entries(&self) -> bool {
        matches!(self.kind, FileSystemSandboxKind::Restricted)
            && self.entries.iter().any(|entry| {
                if entry.access.can_write() {
                    return false;
                }

                match &entry.path {
                    FileSystemPath::Path { .. } => !self.has_same_target_write_override(entry),
                    FileSystemPath::GlobPattern { .. } => true,
                    FileSystemPath::Special { value } => match value {
                        FileSystemSpecialPath::Root => entry.access == FileSystemAccessMode::Deny,
                        FileSystemSpecialPath::Minimal | FileSystemSpecialPath::Unknown { .. } => {
                            false
                        }
                        _ => !self.has_same_target_write_override(entry),
                    },
                }
            })
    }

    /// Returns true when a higher-priority `write` entry targets the same
    /// location as `entry`, so `entry` cannot narrow effective write access.
    fn has_same_target_write_override(&self, entry: &FileSystemSandboxEntry) -> bool {
        self.entries.iter().any(|candidate| {
            candidate.access.can_write()
                && candidate.access > entry.access
                && file_system_paths_share_target(&candidate.path, &entry.path)
        })
    }

    /// Filesystem policy matching `WorkspaceWrite` semantics without requiring
    /// callers to construct a legacy [`SandboxPolicy`] first.
    pub fn workspace_write(
        writable_roots: &[AbsolutePathBuf],
        exclude_tmpdir_env_var: bool,
        exclude_slash_tmp: bool,
    ) -> Self {
        let mut entries = workspace_write_core_entries(
            writable_roots.iter().cloned(),
            exclude_tmpdir_env_var,
            exclude_slash_tmp,
        );

        append_default_read_only_project_root_subpath_if_no_explicit_rule(&mut entries, ".git");
        append_default_read_only_project_root_subpath_if_no_explicit_rule(&mut entries, ".agents");
        append_default_read_only_project_root_subpath_if_no_explicit_rule(&mut entries, ".devo");
        append_default_read_only_project_root_subpath_if_no_explicit_rule(&mut entries, ".codex");
        for writable_root in writable_roots {
            for protected_path in default_read_only_subpaths_for_writable_root(
                writable_root,
                /*protect_missing_legacy_agent_dir*/ false,
            ) {
                append_default_read_only_path_if_no_explicit_rule(&mut entries, protected_path);
            }
        }

        FileSystemSandboxPolicy::restricted(entries)
    }

    /// Returns true when filesystem reads are unrestricted.
    pub fn has_full_disk_read_access(&self) -> bool {
        match self.kind {
            FileSystemSandboxKind::Unrestricted | FileSystemSandboxKind::ExternalSandbox => true,
            FileSystemSandboxKind::Restricted => {
                self.has_root_access(FileSystemAccessMode::can_read)
                    && !self.has_denied_read_restrictions()
            }
        }
    }

    /// Returns true when filesystem writes are unrestricted.
    pub fn has_full_disk_write_access(&self) -> bool {
        match self.kind {
            FileSystemSandboxKind::Unrestricted | FileSystemSandboxKind::ExternalSandbox => true,
            FileSystemSandboxKind::Restricted => {
                self.has_root_access(FileSystemAccessMode::can_write)
                    && !self.has_write_narrowing_entries()
            }
        }
    }

    /// Returns true when platform-default readable roots should be included.
    pub fn include_platform_defaults(&self) -> bool {
        !self.has_full_disk_read_access()
            && matches!(self.kind, FileSystemSandboxKind::Restricted)
            && self.entries.iter().any(|entry| {
                matches!(
                    &entry.path,
                    FileSystemPath::Special { value }
                        if matches!(value, FileSystemSpecialPath::Minimal)
                            && entry.access.can_read()
                )
            })
    }

    pub fn resolve_access_with_cwd(&self, path: &Path, cwd: &Path) -> FileSystemAccessMode {
        match self.kind {
            FileSystemSandboxKind::Unrestricted | FileSystemSandboxKind::ExternalSandbox => {
                return FileSystemAccessMode::Write;
            }
            FileSystemSandboxKind::Restricted => {}
        }

        let Some(path) = resolve_candidate_path(path, cwd) else {
            return FileSystemAccessMode::Deny;
        };

        self.resolved_entries_with_cwd(cwd)
            .into_iter()
            .filter(|entry| path.as_path().starts_with(entry.path.as_path()))
            .max_by_key(resolved_entry_precedence)
            .map(|entry| entry.access)
            .unwrap_or(FileSystemAccessMode::Deny)
    }

    pub fn can_read_path_with_cwd(&self, path: &Path, cwd: &Path) -> bool {
        self.resolve_access_with_cwd(path, cwd).can_read()
    }

    pub fn can_write_path_with_cwd(&self, path: &Path, cwd: &Path) -> bool {
        self.resolve_access_with_cwd(path, cwd).can_write()
            && (self.has_full_disk_write_access() || !self.is_metadata_write_denied(path, cwd))
    }

    fn is_metadata_write_denied(&self, path: &Path, cwd: &Path) -> bool {
        if !matches!(self.kind, FileSystemSandboxKind::Restricted) {
            return false;
        }
        let Some(target) = resolve_candidate_path(path, cwd) else {
            return true;
        };
        metadata_child_of_writable_root(self, target.as_path(), cwd).is_some_and(
            |(protected_metadata_path, _)| {
                !has_explicit_write_entry_for_metadata_path(
                    self,
                    &protected_metadata_path,
                    target.as_path(),
                    cwd,
                )
            },
        )
    }

    /// Replaces symbolic `:workspace_roots` entries with concrete entries for
    /// each workspace root.
    pub fn materialize_project_roots_with_workspace_roots(
        mut self,
        workspace_roots: &[AbsolutePathBuf],
    ) -> Self {
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in self.entries {
            match entry.path {
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::ProjectRoots { subpath },
                } => {
                    entries.extend(workspace_roots.iter().map(|root| {
                        path_entry(
                            match subpath.as_ref() {
                                Some(subpath) => AbsolutePathBuf::resolve_path_against_base(
                                    subpath,
                                    root.as_path(),
                                ),
                                None => root.clone(),
                            },
                            entry.access,
                        )
                    }));
                }
                FileSystemPath::GlobPattern { pattern } => {
                    if let Some(subpath) = parse_project_roots_glob_pattern(&pattern) {
                        entries.extend(workspace_roots.iter().map(|root| {
                            sandbox_entry(
                                FileSystemPath::GlobPattern {
                                    pattern: resolve_project_roots_glob_pattern(subpath, root),
                                },
                                entry.access,
                            )
                        }));
                    } else {
                        entries.push(sandbox_entry(
                            FileSystemPath::GlobPattern { pattern },
                            entry.access,
                        ));
                    }
                }
                FileSystemPath::Path { path } => {
                    entries.push(path_entry(path, entry.access));
                }
                FileSystemPath::Special { value } => {
                    entries.push(FileSystemSandboxEntry {
                        path: FileSystemPath::Special { value },
                        access: entry.access,
                    });
                }
            }
        }
        self.entries = entries;
        self
    }

    /// Returns the explicit readable roots resolved against the provided cwd.
    pub fn get_readable_roots_with_cwd(&self, cwd: &Path) -> Vec<AbsolutePathBuf> {
        if self.has_full_disk_read_access() {
            return Vec::new();
        }

        dedup_absolute_paths(
            self.resolved_entries_with_cwd(cwd)
                .into_iter()
                .filter(|entry| entry.access.can_read())
                .filter(|entry| self.can_read_path_with_cwd(entry.path.as_path(), cwd))
                .map(|entry| entry.path)
                .collect(),
            /*normalize_effective_paths*/ true,
        )
    }

    /// Returns the writable roots together with read-only carveouts resolved
    /// against the provided cwd.
    pub fn get_writable_roots_with_cwd(&self, cwd: &Path) -> Vec<WritableRoot> {
        if self.has_full_disk_write_access() {
            return Vec::new();
        }

        let resolved_entries = self.resolved_entries_with_cwd(cwd);
        let writable_entries: Vec<AbsolutePathBuf> = resolved_entries
            .iter()
            .filter(|entry| entry.access.can_write())
            .filter(|entry| self.can_write_path_with_cwd(entry.path.as_path(), cwd))
            .map(|entry| entry.path.clone())
            .collect();

        dedup_absolute_paths(
            writable_entries.clone(),
            /*normalize_effective_paths*/ true,
        )
        .into_iter()
        .map(|root| {
            // Filesystem-root policies stay in their effective canonical form
            // so root-wide aliases do not create duplicate top-level masks.
            // Example: keep `/var/...` normalized under `/` instead of
            // materializing both `/var/...` and `/private/var/...`.
            // Nested symlink paths under a writable root stay logical so
            // downstream sandboxes can still bind the real target while
            // masking the user-visible symlink inode when needed.
            let preserve_raw_carveout_paths = root.as_path().parent().is_some();
            let raw_writable_roots: Vec<&AbsolutePathBuf> = writable_entries
                .iter()
                .filter(|path| normalize_effective_absolute_path((*path).clone()) == root)
                .collect();
            let protected_metadata_names =
                protected_metadata_names_for_writable_root(self, &root, &raw_writable_roots, cwd);
            let protect_missing_legacy_agent_dir = AbsolutePathBuf::from_absolute_path(cwd)
                .ok()
                .is_some_and(|cwd| normalize_effective_absolute_path(cwd) == root);
            let mut read_only_subpaths: Vec<AbsolutePathBuf> =
                default_read_only_subpaths_for_writable_root(
                    &root,
                    protect_missing_legacy_agent_dir,
                )
                .into_iter()
                .filter(|path| !has_explicit_resolved_path_entry(&resolved_entries, path))
                .collect();
            // Narrower explicit non-write entries carve out broader writable roots.
            // More specific write entries still remain writable because they appear
            // as separate WritableRoot values and are checked independently.
            // Preserve symlink path components that live under the writable root
            // so downstream sandboxes can still mask the symlink inode itself.
            // Example: if `<root>/.codex -> <root>/decoy`, bwrap must still see
            // `<root>/.codex`, not only the resolved `<root>/decoy`.
            read_only_subpaths.extend(
                resolved_entries
                    .iter()
                    .filter(|entry| !entry.access.can_write())
                    .filter(|entry| !self.can_write_path_with_cwd(entry.path.as_path(), cwd))
                    .filter_map(|entry| {
                        let effective_path = normalize_effective_absolute_path(entry.path.clone());
                        // Preserve the literal in-root path whenever the
                        // carveout itself lives under this writable root, even
                        // if following symlinks would resolve back to the root
                        // or escape outside it. Downstream sandboxes need that
                        // raw path so they can mask the symlink inode itself.
                        // Examples:
                        // - `<root>/linked-private -> <root>/decoy-private`
                        // - `<root>/linked-private -> /tmp/outside-private`
                        // - `<root>/alias-root -> <root>`
                        let raw_carveout_path = if preserve_raw_carveout_paths {
                            if entry.path == root {
                                None
                            } else if entry.path.as_path().starts_with(root.as_path()) {
                                Some(entry.path.clone())
                            } else {
                                raw_writable_roots.iter().find_map(|raw_root| {
                                    let suffix = entry
                                        .path
                                        .as_path()
                                        .strip_prefix(raw_root.as_path())
                                        .ok()?;
                                    if suffix.as_os_str().is_empty() {
                                        return None;
                                    }
                                    Some(root.join(suffix))
                                })
                            }
                        } else {
                            None
                        };

                        if let Some(raw_carveout_path) = raw_carveout_path {
                            return Some(raw_carveout_path);
                        }

                        if effective_path == root
                            || !effective_path.as_path().starts_with(root.as_path())
                        {
                            return None;
                        }

                        Some(effective_path)
                    }),
            );
            WritableRoot {
                protected_metadata_names,
                root,
                // Preserve literal in-root protected paths like `.git` and
                // `.codex` so downstream sandboxes can still detect and mask
                // the symlink itself instead of only its resolved target.
                read_only_subpaths: dedup_absolute_paths(
                    read_only_subpaths,
                    /*normalize_effective_paths*/ false,
                ),
            }
        })
        .collect()
    }

    /// Returns explicit unreadable roots resolved against the provided cwd.
    pub fn get_unreadable_roots_with_cwd(&self, cwd: &Path) -> Vec<AbsolutePathBuf> {
        if !matches!(self.kind, FileSystemSandboxKind::Restricted) {
            return Vec::new();
        }

        let root = AbsolutePathBuf::from_absolute_path(cwd)
            .ok()
            .map(|cwd| absolute_root_path_for_cwd(&cwd));

        dedup_absolute_paths(
            self.resolved_entries_with_cwd(cwd)
                .iter()
                .filter(|entry| entry.access == FileSystemAccessMode::Deny)
                .filter(|entry| !self.can_read_path_with_cwd(entry.path.as_path(), cwd))
                // Restricted policies already deny reads outside explicit allow roots,
                // so materializing the filesystem root here would erase narrower
                // readable carveouts when downstream sandboxes apply deny masks last.
                .filter(|entry| root.as_ref() != Some(&entry.path))
                .map(|entry| entry.path.clone())
                .collect(),
            /*normalize_effective_paths*/ true,
        )
    }

    /// Returns unreadable glob patterns resolved against the provided cwd.
    pub fn get_unreadable_globs_with_cwd(&self, cwd: &Path) -> Vec<String> {
        if !matches!(self.kind, FileSystemSandboxKind::Restricted) {
            return Vec::new();
        }

        let mut patterns = self
            .entries
            .iter()
            .filter(|entry| entry.access == FileSystemAccessMode::Deny)
            .filter_map(|entry| match &entry.path {
                FileSystemPath::GlobPattern { pattern } => {
                    Some(AbsolutePathBuf::resolve_path_against_base(pattern, cwd))
                }
                FileSystemPath::Path { .. } | FileSystemPath::Special { .. } => None,
            })
            .map(|pattern| pattern.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        patterns.sort();
        patterns.dedup();
        patterns
    }

    pub fn to_legacy_sandbox_policy(
        &self,
        network_policy: NetworkSandboxPolicy,
        cwd: &Path,
    ) -> io::Result<SandboxPolicy> {
        Ok(match self.kind {
            FileSystemSandboxKind::ExternalSandbox => SandboxPolicy::ExternalSandbox {
                network_access: if network_policy.is_enabled() {
                    NetworkAccess::Enabled
                } else {
                    NetworkAccess::Restricted
                },
            },
            FileSystemSandboxKind::Unrestricted => {
                if network_policy.is_enabled() {
                    SandboxPolicy::DangerFullAccess
                } else {
                    SandboxPolicy::ExternalSandbox {
                        network_access: NetworkAccess::Restricted,
                    }
                }
            }
            FileSystemSandboxKind::Restricted => {
                let cwd_absolute = AbsolutePathBuf::from_absolute_path(cwd).ok();
                let has_full_disk_write_access = self.has_full_disk_write_access();
                let mut workspace_root_writable = false;
                let mut writable_roots = Vec::new();
                let mut tmpdir_writable = false;
                let mut slash_tmp_writable = false;
                let mut unbridgeable_root_write = false;

                for entry in &self.entries {
                    match &entry.path {
                        FileSystemPath::GlobPattern { .. } => {}
                        FileSystemPath::Path { path } => {
                            if entry.access.can_write() {
                                if cwd_absolute.as_ref().is_some_and(|cwd| cwd == path) {
                                    workspace_root_writable = true;
                                } else {
                                    writable_roots.push(path.clone().into());
                                }
                            }
                        }
                        FileSystemPath::Special { value } => match value {
                            FileSystemSpecialPath::Root => match entry.access {
                                FileSystemAccessMode::Deny => {}
                                FileSystemAccessMode::Read => {}
                                FileSystemAccessMode::Write => {
                                    unbridgeable_root_write = true;
                                }
                            },
                            FileSystemSpecialPath::Minimal => {}
                            FileSystemSpecialPath::ProjectRoots { subpath } => {
                                if subpath.is_none() && entry.access.can_write() {
                                    workspace_root_writable = true;
                                } else if let Some(path) =
                                    resolve_file_system_special_path(value, cwd_absolute.as_ref())
                                    && entry.access.can_write()
                                {
                                    writable_roots.push(path);
                                }
                            }
                            FileSystemSpecialPath::Tmpdir => {
                                if entry.access.can_write() {
                                    tmpdir_writable = true;
                                }
                            }
                            FileSystemSpecialPath::SlashTmp => {
                                if entry.access.can_write() {
                                    slash_tmp_writable = true;
                                }
                            }
                            FileSystemSpecialPath::Unknown { .. } => {}
                        },
                    }
                }

                if has_full_disk_write_access {
                    return Ok(if network_policy.is_enabled() {
                        SandboxPolicy::DangerFullAccess
                    } else {
                        SandboxPolicy::ExternalSandbox {
                            network_access: NetworkAccess::Restricted,
                        }
                    });
                }

                if workspace_root_writable {
                    SandboxPolicy::WorkspaceWrite {
                        writable_roots: dedup_absolute_paths(
                            writable_roots,
                            /*normalize_effective_paths*/ false,
                        ),
                        network_access: network_policy.is_enabled(),
                        exclude_tmpdir_env_var: !tmpdir_writable,
                        exclude_slash_tmp: !slash_tmp_writable,
                    }
                } else if unbridgeable_root_write
                    || !writable_roots.is_empty()
                    || tmpdir_writable
                    || slash_tmp_writable
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "permissions profile requests filesystem writes outside the workspace root, which is not supported until the runtime enforces FileSystemSandboxPolicy directly",
                    ));
                } else {
                    SandboxPolicy::ReadOnly {
                        network_access: network_policy.is_enabled(),
                    }
                }
            }
        })
    }

    fn resolved_entries_with_cwd(&self, cwd: &Path) -> Vec<ResolvedFileSystemEntry> {
        let cwd_absolute = AbsolutePathBuf::from_absolute_path(cwd).ok();
        self.entries
            .iter()
            .filter_map(|entry| {
                resolve_entry_path(&entry.path, cwd_absolute.as_ref()).map(|path| {
                    ResolvedFileSystemEntry {
                        path,
                        access: entry.access,
                    }
                })
            })
            .collect()
    }
}

fn resolve_file_system_path(
    path: &FileSystemPath,
    cwd: Option<&AbsolutePathBuf>,
) -> Option<AbsolutePathBuf> {
    match path {
        FileSystemPath::Path { path } => Some(path.clone().into()),
        FileSystemPath::GlobPattern { .. } => None,
        FileSystemPath::Special { value } => resolve_file_system_special_path(value, cwd),
    }
}

fn resolve_entry_path(
    path: &FileSystemPath,
    cwd: Option<&AbsolutePathBuf>,
) -> Option<AbsolutePathBuf> {
    match path {
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        } => cwd.map(absolute_root_path_for_cwd),
        _ => resolve_file_system_path(path, cwd),
    }
}

fn parse_project_roots_glob_pattern(pattern: &str) -> Option<&Path> {
    pattern
        .strip_prefix(PROJECT_ROOTS_GLOB_PATTERN_PREFIX)
        .map(Path::new)
}

fn resolve_project_roots_glob_pattern(subpath: &Path, root: &AbsolutePathBuf) -> String {
    AbsolutePathBuf::resolve_path_against_base(subpath, root.as_path())
        .to_string_lossy()
        .into_owned()
}

fn resolve_candidate_path(path: &Path, cwd: &Path) -> Option<AbsolutePathBuf> {
    if path.is_absolute() {
        AbsolutePathBuf::from_absolute_path(path).ok()
    } else {
        Some(AbsolutePathBuf::from_absolute_path(cwd).ok()?.join(path))
    }
}

/// Returns true when two config paths refer to the same exact target before
/// any prefix matching is applied.
///
/// This is intentionally narrower than full path resolution: it only answers
/// the "can one entry shadow another at the same specificity?" question used
/// by `has_write_narrowing_entries`.
fn file_system_paths_share_target(left: &FileSystemPath, right: &FileSystemPath) -> bool {
    match (left, right) {
        (FileSystemPath::Path { path: left }, FileSystemPath::Path { path: right }) => {
            left == right
        }
        (FileSystemPath::Special { value: left }, FileSystemPath::Special { value: right }) => {
            special_paths_share_target(left, right)
        }
        (FileSystemPath::Path { path }, FileSystemPath::Special { value })
        | (FileSystemPath::Special { value }, FileSystemPath::Path { path }) => {
            special_path_matches_absolute_path(value, path)
        }
        (
            FileSystemPath::GlobPattern { pattern: left },
            FileSystemPath::GlobPattern { pattern: right },
        ) => left == right,
        (FileSystemPath::GlobPattern { .. }, _) | (_, FileSystemPath::GlobPattern { .. }) => false,
    }
}

/// Compares special-path tokens that resolve to the same concrete target
/// without needing a cwd.
fn special_paths_share_target(left: &FileSystemSpecialPath, right: &FileSystemSpecialPath) -> bool {
    match (left, right) {
        (FileSystemSpecialPath::Root, FileSystemSpecialPath::Root)
        | (FileSystemSpecialPath::Minimal, FileSystemSpecialPath::Minimal)
        | (FileSystemSpecialPath::Tmpdir, FileSystemSpecialPath::Tmpdir)
        | (FileSystemSpecialPath::SlashTmp, FileSystemSpecialPath::SlashTmp) => true,
        (
            FileSystemSpecialPath::ProjectRoots { subpath: left },
            FileSystemSpecialPath::ProjectRoots { subpath: right },
        ) => left == right,
        (
            FileSystemSpecialPath::Unknown {
                path: left,
                subpath: left_subpath,
            },
            FileSystemSpecialPath::Unknown {
                path: right,
                subpath: right_subpath,
            },
        ) => left == right && left_subpath == right_subpath,
        _ => false,
    }
}

/// Matches cwd-independent special paths against absolute `Path` entries when
/// they name the same location.
///
/// We intentionally only fold the special paths whose concrete meaning is
/// stable without a cwd, such as `/` and `/tmp`.
fn special_path_matches_absolute_path(
    value: &FileSystemSpecialPath,
    path: &AbsolutePathBuf,
) -> bool {
    match value {
        FileSystemSpecialPath::Root => path.as_path().parent().is_none(),
        FileSystemSpecialPath::SlashTmp => path.as_path() == Path::new("/tmp"),
        _ => false,
    }
}

/// Orders resolved entries so the most specific path wins first, then applies
/// the access tie-breaker from [`FileSystemAccessMode`].
fn resolved_entry_precedence(entry: &ResolvedFileSystemEntry) -> (usize, FileSystemAccessMode) {
    let specificity = entry.path.as_path().components().count();
    (specificity, entry.access)
}

fn absolute_root_path_for_cwd(cwd: &AbsolutePathBuf) -> AbsolutePathBuf {
    let root = cwd
        .as_path()
        .ancestors()
        .last()
        .unwrap_or_else(|| panic!("cwd must have a filesystem root"));
    AbsolutePathBuf::from_absolute_path(root)
        .unwrap_or_else(|err| panic!("cwd root must be an absolute path: {err}"))
}

fn normalized_and_canonical_candidates(path: &Path) -> Vec<PathBuf> {
    // Compare the lexical absolute form plus the canonical target when it
    // exists. Missing paths still need the lexical candidate so future-created
    // denied paths remain blocked by direct tool checks.
    let mut candidates = Vec::new();

    if let Ok(normalized) = AbsolutePathBuf::from_absolute_path(path) {
        push_unique(&mut candidates, normalized.to_path_buf());
    } else {
        push_unique(&mut candidates, path.to_path_buf());
    }

    if let Ok(canonical) = path.canonicalize()
        && let Ok(canonical_absolute) = AbsolutePathBuf::from_absolute_path(canonical)
    {
        push_unique(&mut candidates, canonical_absolute.to_path_buf());
    }

    candidates
}

fn push_unique(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn build_glob_matcher(pattern: &str) -> Result<GlobMatcher, String> {
    // Keep `*` and `?` within a single path component and preserve an unclosed
    // `[` as a literal so matcher behavior stays aligned with config parsing.
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .allow_unclosed_class(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|err| err.to_string())
}

fn resolve_file_system_special_path(
    value: &FileSystemSpecialPath,
    cwd: Option<&AbsolutePathBuf>,
) -> Option<AbsolutePathBuf> {
    match value {
        FileSystemSpecialPath::Root
        | FileSystemSpecialPath::Minimal
        | FileSystemSpecialPath::Unknown { .. } => None,
        FileSystemSpecialPath::ProjectRoots { subpath } => {
            let cwd = cwd?;
            match subpath.as_ref() {
                Some(subpath) => Some(AbsolutePathBuf::resolve_path_against_base(
                    subpath,
                    cwd.as_path(),
                )),
                None => Some(cwd.clone()),
            }
        }
        FileSystemSpecialPath::Tmpdir => {
            let tmpdir = std::env::var_os("TMPDIR")?;
            if tmpdir.is_empty() {
                None
            } else {
                let tmpdir = AbsolutePathBuf::from_absolute_path(PathBuf::from(tmpdir)).ok()?;
                Some(tmpdir)
            }
        }
        FileSystemSpecialPath::SlashTmp => {
            #[allow(clippy::expect_used)]
            let slash_tmp = AbsolutePathBuf::from_absolute_path("/tmp").expect("/tmp is absolute");
            if !slash_tmp.as_path().is_dir() {
                return None;
            }
            Some(slash_tmp)
        }
    }
}

fn dedup_absolute_paths(
    paths: Vec<AbsolutePathBuf>,
    normalize_effective_paths: bool,
) -> Vec<AbsolutePathBuf> {
    let mut deduped = Vec::with_capacity(paths.len());
    let mut seen = HashSet::new();
    for path in paths {
        let dedup_path = if normalize_effective_paths {
            normalize_effective_absolute_path(path)
        } else {
            path
        };
        if seen.insert(dedup_path.to_path_buf()) {
            deduped.push(dedup_path);
        }
    }
    deduped
}

fn normalize_effective_absolute_path(path: AbsolutePathBuf) -> AbsolutePathBuf {
    let raw_path = path.to_path_buf();
    for ancestor in raw_path.ancestors() {
        if std::fs::symlink_metadata(ancestor).is_err() {
            continue;
        }
        let Ok(normalized_ancestor) = canonicalize_preserving_symlinks(ancestor) else {
            continue;
        };
        let Ok(suffix) = raw_path.strip_prefix(ancestor) else {
            continue;
        };
        if let Ok(normalized_path) =
            AbsolutePathBuf::from_absolute_path(normalized_ancestor.join(suffix))
        {
            return normalized_path;
        }
    }
    path
}

pub fn default_read_only_subpaths_for_writable_root(
    writable_root: &AbsolutePathBuf,
    protect_missing_legacy_agent_dir: bool,
) -> Vec<AbsolutePathBuf> {
    let mut subpaths: Vec<AbsolutePathBuf> = Vec::new();
    let top_level_git = writable_root.join(PROTECTED_METADATA_GIT_PATH_NAME);
    // This applies to typical repos (directory .git), worktrees/submodules
    // (file .git with gitdir pointer), and bare repos when the gitdir is the
    // writable root itself.
    let top_level_git_is_file = top_level_git.as_path().is_file();
    let top_level_git_is_dir = top_level_git.as_path().is_dir();
    let should_protect_top_level = top_level_git_is_dir || top_level_git_is_file;
    if should_protect_top_level {
        if top_level_git_is_file
            && is_git_pointer_file(&top_level_git)
            && let Some(gitdir) = resolve_gitdir_from_file(&top_level_git)
        {
            subpaths.push(gitdir);
        }
        subpaths.push(top_level_git);
    }

    let top_level_agents = writable_root.join(PROTECTED_METADATA_AGENTS_PATH_NAME);
    if top_level_agents.as_path().is_dir() {
        subpaths.push(top_level_agents);
    }

    // Keep top-level project metadata under .devo / .codex read-only to the
    // agent by default. For the workspace root itself, protect them even before
    // the directory exists so first-time creation still goes through the
    // protected-path approval flow.
    let top_level_devo = writable_root.join(PROTECTED_METADATA_DEVO_PATH_NAME);
    if protect_missing_legacy_agent_dir || top_level_devo.as_path().is_dir() {
        subpaths.push(top_level_devo);
    }
    let top_level_legacy_agent_dir = writable_root.join(PROTECTED_METADATA_LEGACY_AGENT_PATH_NAME);
    if protect_missing_legacy_agent_dir || top_level_legacy_agent_dir.as_path().is_dir() {
        subpaths.push(top_level_legacy_agent_dir);
    }

    dedup_absolute_paths(subpaths, /*normalize_effective_paths*/ false)
}

fn append_default_read_only_project_root_subpath_if_no_explicit_rule(
    entries: &mut Vec<FileSystemSandboxEntry>,
    subpath: impl Into<String>,
) {
    append_default_read_only_entry_if_no_explicit_rule(
        entries,
        FileSystemPath::Special {
            value: FileSystemSpecialPath::project_roots(Some(subpath.into())),
        },
    );
}

fn append_default_read_only_path_if_no_explicit_rule(
    entries: &mut Vec<FileSystemSandboxEntry>,
    path: AbsolutePathBuf,
) {
    append_default_read_only_entry_if_no_explicit_rule(entries, FileSystemPath::from_path(path));
}

fn append_default_read_only_entry_if_no_explicit_rule(
    entries: &mut Vec<FileSystemSandboxEntry>,
    path: FileSystemPath,
) {
    if entries
        .iter()
        .any(|entry| file_system_paths_share_target(&entry.path, &path))
    {
        return;
    }

    entries.push(FileSystemSandboxEntry {
        path,
        access: FileSystemAccessMode::Read,
    });
}

fn has_explicit_resolved_path_entry(
    entries: &[ResolvedFileSystemEntry],
    path: &AbsolutePathBuf,
) -> bool {
    entries.iter().any(|entry| &entry.path == path)
}

fn metadata_path_name(name: &OsStr) -> Option<&'static str> {
    PROTECTED_METADATA_PATH_NAMES
        .iter()
        .copied()
        .find(|metadata_name| name == OsStr::new(metadata_name))
}

fn metadata_child_of_writable_root(
    policy: &FileSystemSandboxPolicy,
    target: &Path,
    cwd: &Path,
) -> Option<(AbsolutePathBuf, &'static str)> {
    policy
        .resolved_entries_with_cwd(cwd)
        .iter()
        .filter(|entry| entry.access.can_write())
        .filter_map(|entry| {
            let relative_path = target.strip_prefix(entry.path.as_path()).ok()?;
            let first_component = relative_path.components().next()?;
            let metadata_name = metadata_path_name(first_component.as_os_str())?;
            Some((entry.path.join(metadata_name), metadata_name))
        })
        .next()
}

fn protected_metadata_names_for_writable_root(
    policy: &FileSystemSandboxPolicy,
    root: &AbsolutePathBuf,
    raw_writable_roots: &[&AbsolutePathBuf],
    cwd: &Path,
) -> Vec<String> {
    let mut protected_names = Vec::new();
    for metadata_name in PROTECTED_METADATA_PATH_NAMES {
        let mut metadata_paths = vec![root.join(*metadata_name)];
        metadata_paths.extend(
            raw_writable_roots
                .iter()
                .map(|raw_root| raw_root.join(*metadata_name)),
        );

        if metadata_paths
            .iter()
            .all(|metadata_path| !policy.can_write_path_with_cwd(metadata_path.as_path(), cwd))
        {
            protected_names.push((*metadata_name).to_string());
        }
    }
    protected_names
}

fn has_explicit_write_entry_for_metadata_path(
    policy: &FileSystemSandboxPolicy,
    protected_metadata_path: &AbsolutePathBuf,
    target: &Path,
    cwd: &Path,
) -> bool {
    policy.resolved_entries_with_cwd(cwd).iter().any(|entry| {
        entry.access.can_write()
            && target.starts_with(entry.path.as_path())
            && entry
                .path
                .as_path()
                .starts_with(protected_metadata_path.as_path())
    })
}

fn is_git_pointer_file(path: &AbsolutePathBuf) -> bool {
    path.as_path().is_file()
        && path.as_path().file_name() == Some(OsStr::new(PROTECTED_METADATA_GIT_PATH_NAME))
}

fn resolve_gitdir_from_file(dot_git: &AbsolutePathBuf) -> Option<AbsolutePathBuf> {
    let contents = match std::fs::read_to_string(dot_git.as_path()) {
        Ok(contents) => contents,
        Err(err) => {
            error!(
                "Failed to read {path} for gitdir pointer: {err}",
                path = dot_git.as_path().display()
            );
            return None;
        }
    };

    let trimmed = contents.trim();
    let (_, gitdir_raw) = match trimmed.split_once(':') {
        Some((prefix, gitdir_raw)) if prefix.trim() == "gitdir" => (prefix, gitdir_raw),
        _ => {
            error!(
                "Expected {path} to contain a gitdir pointer, but it did not match `gitdir: <path>`.",
                path = dot_git.as_path().display()
            );
            return None;
        }
    };
    let gitdir_raw = gitdir_raw.trim();
    if gitdir_raw.is_empty() {
        error!(
            "Expected {path} to contain a gitdir pointer, but it was empty.",
            path = dot_git.as_path().display()
        );
        return None;
    }
    let base = match dot_git.as_path().parent() {
        Some(base) => base,
        None => {
            error!(
                "Unable to resolve parent directory for {path}.",
                path = dot_git.as_path().display()
            );
            return None;
        }
    };
    let gitdir_path = AbsolutePathBuf::resolve_path_against_base(gitdir_raw, base);
    if !gitdir_path.as_path().exists() {
        error!(
            "Resolved gitdir path {path} does not exist.",
            path = gitdir_path.as_path().display()
        );
        return None;
    }
    Some(gitdir_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    #[cfg(unix)]
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn project(subpath: Option<&str>, access: FileSystemAccessMode) -> FileSystemSandboxEntry {
        special_entry(
            FileSystemSpecialPath::project_roots(subpath.map(str::to_string)),
            access,
        )
    }

    fn deny_policy(path: &Path) -> FileSystemSandboxPolicy {
        FileSystemSandboxPolicy::restricted(vec![path_entry(
            AbsolutePathBuf::try_from(path).expect("absolute deny path"),
            FileSystemAccessMode::Deny,
        )])
    }

    fn unreadable_glob_entry(pattern: String) -> FileSystemSandboxEntry {
        sandbox_entry(
            FileSystemPath::GlobPattern { pattern },
            FileSystemAccessMode::Deny,
        )
    }

    fn default_policy_with_unreadable_glob(pattern: String) -> FileSystemSandboxPolicy {
        let mut policy = FileSystemSandboxPolicy::default();
        policy.entries.push(unreadable_glob_entry(pattern));
        policy
    }

    fn is_read_denied(
        path: &Path,
        file_system_sandbox_policy: &FileSystemSandboxPolicy,
        cwd: &Path,
    ) -> bool {
        ReadDenyMatcher::try_new(file_system_sandbox_policy, cwd)
            .expect("matcher")
            .is_some_and(|matcher| matcher.is_read_denied(path))
    }

    #[cfg(unix)]
    const SYMLINKED_TMPDIR_TEST_ENV: &str = "DEVO_PROTOCOL_TEST_SYMLINKED_TMPDIR";

    #[cfg(unix)]
    fn symlink_dir(original: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[test]
    fn unknown_special_paths_are_ignored_by_legacy_bridge() -> std::io::Result<()> {
        let policy = FileSystemSandboxPolicy::restricted(vec![
            special_entry(FileSystemSpecialPath::Root, FileSystemAccessMode::Read),
            special_entry(
                FileSystemSpecialPath::unknown(":future_special_path", /*subpath*/ None),
                FileSystemAccessMode::Write,
            ),
        ]);

        let sandbox_policy = policy.to_legacy_sandbox_policy(
            NetworkSandboxPolicy::Restricted,
            Path::new("/tmp/workspace"),
        )?;

        assert_eq!(
            sandbox_policy,
            SandboxPolicy::ReadOnly {
                network_access: false,
            }
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn writable_roots_apply_default_metadata_protections() {
        let cwd = TempDir::new().expect("tempdir");
        let expected_root = abs(&cwd.path().canonicalize().expect("canonicalize cwd"));
        let explicit_legacy_agent_dir = expected_root.join(".codex");
        let explicit_devo_dir = expected_root.join(".devo");
        let default_policy =
            FileSystemSandboxPolicy::restricted(vec![project(None, FileSystemAccessMode::Write)]);
        assert_single_writable_root(
            &default_policy,
            cwd.path(),
            &expected_root,
            &[explicit_devo_dir, explicit_legacy_agent_dir.clone()],
            &[],
        );

        let explicit_policy = FileSystemSandboxPolicy::restricted(vec![
            project(None, FileSystemAccessMode::Write),
            path_entry(
                explicit_legacy_agent_dir.clone(),
                FileSystemAccessMode::Write,
            ),
        ]);
        let workspace_root = explicit_policy
            .get_writable_roots_with_cwd(cwd.path())
            .into_iter()
            .find(|root| root.root == expected_root)
            .expect("workspace writable root");
        assert!(
            !workspace_root
                .protected_metadata_names
                .contains(&".codex".to_string())
        );
        assert!(
            !workspace_root
                .read_only_subpaths
                .contains(&explicit_legacy_agent_dir)
        );
        assert!(explicit_policy.can_write_path_with_cwd(
            explicit_legacy_agent_dir.join("config.toml").as_path(),
            cwd.path(),
        ));
    }

    #[test]
    fn legacy_workspace_write_projection_preserves_symbolic_project_root() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: true,
            exclude_slash_tmp: true,
        };

        assert_eq!(
            FileSystemSandboxPolicy::from(&policy),
            FileSystemSandboxPolicy::restricted(vec![
                special_entry(FileSystemSpecialPath::Root, FileSystemAccessMode::Read),
                project(None, FileSystemAccessMode::Write),
                project(Some(".git"), FileSystemAccessMode::Read),
                project(Some(".agents"), FileSystemAccessMode::Read),
                project(Some(".devo"), FileSystemAccessMode::Read),
                project(Some(".codex"), FileSystemAccessMode::Read),
            ])
        );
    }

    #[test]
    fn legacy_current_working_directory_special_path_deserializes_as_project_roots()
    -> serde_json::Result<()> {
        let value = serde_json::json!({
            "kind": "current_working_directory",
        });

        let special_path = serde_json::from_value::<FileSystemSpecialPath>(value)?;
        assert_eq!(
            special_path,
            FileSystemSpecialPath::project_roots(/*subpath*/ None)
        );
        assert_eq!(
            serde_json::to_value(&special_path)?,
            serde_json::json!({
                "kind": "project_roots",
            })
        );
        Ok(())
    }

    #[test]
    fn filesystem_policy_blocks_protected_metadata_path_writes_by_default() {
        let cwd = TempDir::new().expect("tempdir");
        let dot_git_config = cwd.path().join(".git").join("config");
        let dot_agents_config = cwd.path().join(".agents").join("config");
        let legacy_agent_config = cwd.path().join(".codex").join("config.toml");
        let root = AbsolutePathBuf::from_absolute_path(cwd.path()).expect("absolute cwd");
        let file_system_policy = FileSystemSandboxPolicy::restricted(vec![path_entry(
            root,
            FileSystemAccessMode::Write,
        )]);

        assert!(!file_system_policy.can_write_path_with_cwd(&dot_git_config, cwd.path()));
        assert!(!file_system_policy.can_write_path_with_cwd(&dot_agents_config, cwd.path()));
        assert!(!file_system_policy.can_write_path_with_cwd(&legacy_agent_config, cwd.path()));

        let writable_roots = file_system_policy.get_writable_roots_with_cwd(cwd.path());
        assert_eq!(writable_roots.len(), 1);
        assert_eq!(
            writable_roots[0].protected_metadata_names,
            vec![
                ".git".to_string(),
                ".agents".to_string(),
                ".devo".to_string(),
                ".codex".to_string(),
            ]
        );
        assert!(!writable_roots[0].is_path_writable(&dot_git_config));
        assert!(!writable_roots[0].is_path_writable(&dot_agents_config));
        assert!(!writable_roots[0].is_path_writable(&legacy_agent_config));
    }

    #[cfg(unix)]
    fn abs(path: &Path) -> AbsolutePathBuf {
        AbsolutePathBuf::from_absolute_path(path).expect("absolute path")
    }

    #[cfg(unix)]
    fn assert_single_writable_root(
        policy: &FileSystemSandboxPolicy,
        cwd: &Path,
        expected_root: &AbsolutePathBuf,
        expected_read_only_subpaths: &[AbsolutePathBuf],
        excluded_subpaths: &[AbsolutePathBuf],
    ) {
        let writable_roots = policy.get_writable_roots_with_cwd(cwd);
        assert_eq!(writable_roots.len(), 1);
        assert_eq!(&writable_roots[0].root, expected_root);
        assert_eq!(
            writable_roots[0].read_only_subpaths,
            expected_read_only_subpaths.to_vec()
        );
        for excluded in excluded_subpaths {
            assert!(!writable_roots[0].read_only_subpaths.contains(excluded));
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_symlinked_roots_preserve_runtime_paths() {
        let cwd = TempDir::new().expect("tempdir");
        let real_root = cwd.path().join("real");
        let link_root = cwd.path().join("link");
        fs::create_dir_all(real_root.join("blocked")).expect("create blocked");
        fs::create_dir_all(real_root.join(".codex")).expect("create .codex");
        fs::create_dir_all(real_root.join(".agents")).expect("create .agents");
        symlink_dir(&real_root, &link_root).expect("create symlinked root");

        let link_root = abs(&link_root);
        let link_blocked = link_root.join("blocked");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            path_entry(link_root.clone(), FileSystemAccessMode::Write),
            path_entry(link_blocked.clone(), FileSystemAccessMode::Deny),
        ]);
        assert_eq!(
            policy.get_unreadable_roots_with_cwd(cwd.path()),
            vec![link_blocked.clone()]
        );
        assert_single_writable_root(
            &policy,
            cwd.path(),
            &link_root,
            &[
                link_root.join(".agents"),
                link_root.join(".codex"),
                link_blocked.clone(),
            ],
            &[],
        );

        let project_policy = FileSystemSandboxPolicy::restricted(vec![
            special_entry(FileSystemSpecialPath::Minimal, FileSystemAccessMode::Read),
            project(None, FileSystemAccessMode::Write),
            path_entry(link_blocked.clone(), FileSystemAccessMode::Deny),
        ]);
        assert_eq!(
            project_policy.get_readable_roots_with_cwd(link_root.as_path()),
            vec![link_root.clone()]
        );
        assert_eq!(
            project_policy.get_unreadable_roots_with_cwd(link_root.as_path()),
            vec![link_blocked.clone()]
        );
        assert_single_writable_root(
            &project_policy,
            link_root.as_path(),
            &link_root,
            &[
                link_root.join(".agents"),
                link_root.join(".devo"),
                link_root.join(".codex"),
                link_blocked.clone(),
            ],
            &[],
        );

        let root = cwd.path().join("root");
        let decoy = root.join("decoy-legacy-agent");
        fs::create_dir_all(&decoy).expect("create decoy");
        symlink_dir(&decoy, &root.join(".codex")).expect("create .codex symlink");
        let root = abs(&root);
        let expected_codex = abs(&root
            .as_path()
            .canonicalize()
            .expect("canonicalize root")
            .join(".codex"));
        assert_single_writable_root(
            &FileSystemSandboxPolicy::restricted(vec![path_entry(
                root.clone(),
                FileSystemAccessMode::Write,
            )]),
            cwd.path(),
            &root,
            &[expected_codex],
            &[abs(&decoy.canonicalize().expect("canonicalize decoy"))],
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_symlinked_carveouts_preserve_literal_paths() {
        for (decoy_parent, symlink_under_root) in [
            ("decoy-private", true),
            ("outside-private", false),
            ("alias-root", true),
        ] {
            let cwd = TempDir::new().expect("tempdir");
            let real_root = cwd.path().join("real");
            let link_root = cwd.path().join("link");
            let decoy = if symlink_under_root {
                real_root.join(decoy_parent)
            } else {
                cwd.path().join(decoy_parent)
            };
            let linked_private = real_root.join("linked-private");
            if symlink_under_root && decoy_parent == "alias-root" {
                fs::create_dir_all(&real_root).expect("create root");
                symlink_dir(&real_root, &real_root.join(decoy_parent)).expect("alias symlink");
            } else {
                fs::create_dir_all(&decoy).expect("create decoy");
                if !symlink_under_root {
                    fs::create_dir_all(&real_root).expect("create real root");
                }
                symlink_dir(&real_root, &link_root).expect("create symlinked root");
                symlink_dir(&decoy, &linked_private).expect("create linked-private symlink");
            }

            let link_root = abs(if decoy_parent == "alias-root" {
                &real_root
            } else {
                &link_root
            });
            let link_private = link_root.join(if decoy_parent == "alias-root" {
                "alias-root"
            } else {
                "linked-private"
            });
            let expected_root = if decoy_parent == "alias-root" {
                abs(&real_root.canonicalize().expect("canonicalize root"))
            } else {
                link_root.clone()
            };
            let policy = FileSystemSandboxPolicy::restricted(vec![
                path_entry(link_root.clone(), FileSystemAccessMode::Write),
                path_entry(link_private.clone(), FileSystemAccessMode::Deny),
            ]);
            let excluded = if decoy_parent == "alias-root" {
                vec![]
            } else {
                vec![abs(&decoy.canonicalize().expect("canonicalize decoy"))]
            };
            assert_single_writable_root(
                &policy,
                cwd.path(),
                &expected_root,
                std::slice::from_ref(&link_private),
                &excluded,
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn tmpdir_special_path_preserves_symlinked_tmpdir() {
        if std::env::var_os(SYMLINKED_TMPDIR_TEST_ENV).is_none() {
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .env(SYMLINKED_TMPDIR_TEST_ENV, "1")
                .arg("--exact")
                .arg("permissions::tests::tmpdir_special_path_preserves_symlinked_tmpdir")
                .output()
                .expect("run tmpdir subprocess test");

            assert!(
                output.status.success(),
                "tmpdir subprocess test failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let cwd = TempDir::new().expect("tempdir");
        let real_tmpdir = cwd.path().join("real-tmpdir");
        let link_tmpdir = cwd.path().join("link-tmpdir");
        let blocked = real_tmpdir.join("blocked");
        let legacy_agent_dir = real_tmpdir.join(".codex");

        fs::create_dir_all(&blocked).expect("create blocked");
        fs::create_dir_all(&legacy_agent_dir).expect("create .codex");
        symlink_dir(&real_tmpdir, &link_tmpdir).expect("create symlinked tmpdir");

        let link_blocked =
            AbsolutePathBuf::from_absolute_path(link_tmpdir.join("blocked")).expect("link blocked");
        let expected_root =
            AbsolutePathBuf::from_absolute_path(&link_tmpdir).expect("absolute symlinked tmpdir");
        let expected_blocked = link_blocked.clone();
        let expected_legacy_agent = expected_root.join(".codex");

        unsafe {
            std::env::set_var("TMPDIR", &link_tmpdir);
        }

        let policy = FileSystemSandboxPolicy::restricted(vec![
            special_entry(FileSystemSpecialPath::Tmpdir, FileSystemAccessMode::Write),
            path_entry(link_blocked, FileSystemAccessMode::Deny),
        ]);

        assert_eq!(
            policy.get_unreadable_roots_with_cwd(cwd.path()),
            vec![expected_blocked.clone()]
        );

        let writable_roots = policy.get_writable_roots_with_cwd(cwd.path());
        assert_eq!(writable_roots.len(), 1);
        assert_eq!(writable_roots[0].root, expected_root);
        assert!(
            writable_roots[0]
                .read_only_subpaths
                .contains(&expected_blocked)
        );
        assert!(
            writable_roots[0]
                .read_only_subpaths
                .contains(&expected_legacy_agent)
        );
    }

    #[test]
    fn resolve_access_with_cwd_uses_most_specific_entry() {
        let cwd = TempDir::new().expect("tempdir");
        let docs = AbsolutePathBuf::resolve_path_against_base("docs", cwd.path());
        let docs_private = AbsolutePathBuf::resolve_path_against_base("docs/private", cwd.path());
        let docs_private_public =
            AbsolutePathBuf::resolve_path_against_base("docs/private/public", cwd.path());
        let policy = FileSystemSandboxPolicy::restricted(vec![
            project(None, FileSystemAccessMode::Write),
            path_entry(docs.clone(), FileSystemAccessMode::Read),
            path_entry(docs_private.clone(), FileSystemAccessMode::Deny),
            path_entry(docs_private_public.clone(), FileSystemAccessMode::Write),
        ]);
        for (path, expected) in [
            (cwd.path(), FileSystemAccessMode::Write),
            (docs.as_path(), FileSystemAccessMode::Read),
            (docs_private.as_path(), FileSystemAccessMode::Deny),
            (docs_private_public.as_path(), FileSystemAccessMode::Write),
        ] {
            assert_eq!(policy.resolve_access_with_cwd(path, cwd.path()), expected);
        }
    }

    #[test]
    fn root_access_precedence_cases() {
        let cwd = TempDir::new().expect("tempdir");
        let docs = AbsolutePathBuf::resolve_path_against_base("docs", cwd.path());
        let expected_docs = AbsolutePathBuf::from_absolute_path(
            canonicalize_preserving_symlinks(cwd.path())
                .expect("canonicalize cwd")
                .join("docs"),
        )
        .expect("canonical docs");
        let filesystem_root = AbsolutePathBuf::from_absolute_path(cwd.path())
            .map(|cwd| absolute_root_path_for_cwd(&cwd))
            .expect("resolve filesystem root");

        let narrowed_root_write = FileSystemSandboxPolicy::restricted(vec![
            special_entry(FileSystemSpecialPath::Root, FileSystemAccessMode::Write),
            path_entry(docs.clone(), FileSystemAccessMode::Read),
        ]);
        assert!(!narrowed_root_write.has_full_disk_write_access());
        assert_eq!(
            narrowed_root_write.resolve_access_with_cwd(docs.as_path(), cwd.path()),
            FileSystemAccessMode::Read
        );
        assert!(
            narrowed_root_write
                .to_legacy_sandbox_policy(NetworkSandboxPolicy::Restricted, cwd.path())
                .is_err()
        );

        let root_deny = FileSystemSandboxPolicy::restricted(vec![
            special_entry(FileSystemSpecialPath::Root, FileSystemAccessMode::Deny),
            path_entry(docs.clone(), FileSystemAccessMode::Read),
        ]);
        assert_eq!(
            root_deny.resolve_access_with_cwd(docs.as_path(), cwd.path()),
            FileSystemAccessMode::Read
        );
        assert_eq!(
            root_deny.get_readable_roots_with_cwd(cwd.path()),
            vec![expected_docs]
        );
        assert!(
            root_deny
                .get_unreadable_roots_with_cwd(cwd.path())
                .is_empty()
        );

        let duplicate_root_deny = FileSystemSandboxPolicy::restricted(vec![
            special_entry(FileSystemSpecialPath::Root, FileSystemAccessMode::Write),
            special_entry(FileSystemSpecialPath::Root, FileSystemAccessMode::Deny),
        ]);
        assert!(!duplicate_root_deny.has_full_disk_write_access());
        assert_eq!(
            duplicate_root_deny.resolve_access_with_cwd(filesystem_root.as_path(), cwd.path()),
            FileSystemAccessMode::Deny
        );

        let write_override = FileSystemSandboxPolicy::restricted(vec![
            special_entry(FileSystemSpecialPath::Root, FileSystemAccessMode::Write),
            path_entry(docs.clone(), FileSystemAccessMode::Read),
            path_entry(docs.clone(), FileSystemAccessMode::Write),
        ]);
        assert!(write_override.has_full_disk_write_access());
        assert_eq!(
            write_override.resolve_access_with_cwd(docs.as_path(), cwd.path()),
            FileSystemAccessMode::Write
        );
    }

    #[test]
    fn materialize_project_roots_with_workspace_roots_expands_exact_and_glob_entries() {
        let temp_dir = TempDir::new().expect("tempdir");
        let first = AbsolutePathBuf::from_absolute_path(temp_dir.path().join("first"))
            .expect("resolve first root");
        let second = AbsolutePathBuf::from_absolute_path(temp_dir.path().join("second"))
            .expect("resolve second root");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            project(None, FileSystemAccessMode::Write),
            project(Some(".git"), FileSystemAccessMode::Read),
            sandbox_entry(
                FileSystemPath::GlobPattern {
                    pattern: project_roots_glob_pattern(Path::new("**/*.env")),
                },
                FileSystemAccessMode::Deny,
            ),
        ]);

        let actual =
            policy.materialize_project_roots_with_workspace_roots(&[first.clone(), second.clone()]);

        assert_eq!(
            actual,
            FileSystemSandboxPolicy::restricted(vec![
                path_entry(first.clone(), FileSystemAccessMode::Write),
                path_entry(second.clone(), FileSystemAccessMode::Write),
                path_entry(first.join(".git"), FileSystemAccessMode::Read),
                path_entry(second.join(".git"), FileSystemAccessMode::Read),
                sandbox_entry(
                    FileSystemPath::GlobPattern {
                        pattern: AbsolutePathBuf::resolve_path_against_base(
                            "**/*.env",
                            first.as_path(),
                        )
                        .to_string_lossy()
                        .into_owned(),
                    },
                    FileSystemAccessMode::Deny
                ),
                sandbox_entry(
                    FileSystemPath::GlobPattern {
                        pattern: AbsolutePathBuf::resolve_path_against_base(
                            "**/*.env",
                            second.as_path(),
                        )
                        .to_string_lossy()
                        .into_owned(),
                    },
                    FileSystemAccessMode::Deny
                ),
            ])
        );
    }

    #[test]
    fn file_system_access_mode_orders_by_conflict_precedence() {
        assert!(FileSystemAccessMode::Write > FileSystemAccessMode::Read);
        assert!(FileSystemAccessMode::Deny > FileSystemAccessMode::Write);
    }

    #[test]
    fn exact_path_and_descendants_are_denied() {
        let temp = TempDir::new().expect("tempdir");
        let denied_dir = temp.path().join("denied");
        let nested = denied_dir.join("nested.txt");
        std::fs::create_dir_all(&denied_dir).expect("create denied dir");
        std::fs::write(&nested, "secret").expect("write secret");

        let policy = deny_policy(&denied_dir);
        assert!(is_read_denied(&denied_dir, &policy, temp.path()));
        assert!(is_read_denied(&nested, &policy, temp.path()));
        assert!(!is_read_denied(
            &temp.path().join("other.txt"),
            &policy,
            temp.path()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn canonical_target_matches_denied_symlink_alias() {
        let temp = TempDir::new().expect("tempdir");
        let real_dir = temp.path().join("real");
        let alias_dir = temp.path().join("alias");
        std::fs::create_dir_all(&real_dir).expect("create real dir");
        symlink_dir(&real_dir, &alias_dir).expect("symlink alias");

        let secret = real_dir.join("secret.txt");
        std::fs::write(&secret, "secret").expect("write secret");
        let alias_secret = alias_dir.join("secret.txt");

        let policy = deny_policy(&real_dir);
        assert!(is_read_denied(&alias_secret, &policy, temp.path()));
    }

    fn write_secret(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, "secret").expect("write secret");
    }

    #[test]
    fn literal_patterns_and_globs_are_denied() {
        let temp = TempDir::new().expect("tempdir");
        let literal = temp.path().join("private");
        let other = temp.path().join("notes.txt");
        std::fs::create_dir_all(&literal).expect("create literal dir");
        std::fs::write(&other, "notes").expect("write notes");
        let mut policy = deny_policy(&literal);
        policy.entries.push(unreadable_glob_entry(format!(
            "{}/**/*.txt",
            temp.path().display()
        )));
        assert!(is_read_denied(&literal, &policy, temp.path()));
        assert!(is_read_denied(&other, &policy, temp.path()));
    }

    #[test]
    fn glob_patterns_match_expected_paths() {
        deny_read_glob_cases(&[
            ("{}/private/secret?.txt", &["private/secret1.txt"], &[]),
            (
                "{}/*/file[0-9]?.txt",
                &["app/file42.txt"],
                &["app/nested/file42.txt", "app/file4.txt", "app/fileab.txt"],
            ),
            ("{}/**/*.env", &[".env", "app/.env"], &["app/notes.txt"]),
            ("{}/[", &["["], &["notes.txt"]),
        ]);
    }

    fn deny_read_glob_cases(cases: &[(&str, &[&str], &[&str])]) {
        for (pattern, denied, allowed) in cases {
            let temp = TempDir::new().expect("tempdir");
            let policy = default_policy_with_unreadable_glob(
                pattern.replace("{}", &temp.path().display().to_string()),
            );
            for rel in denied.iter().chain(allowed.iter()) {
                write_secret(&temp.path().join(rel));
            }
            for rel in *denied {
                assert!(
                    is_read_denied(&temp.path().join(rel), &policy, temp.path()),
                    "expected deny for {rel}"
                );
            }
            for rel in *allowed {
                assert!(
                    !is_read_denied(&temp.path().join(rel), &policy, temp.path()),
                    "expected allow for {rel}"
                );
            }
        }
    }
}
