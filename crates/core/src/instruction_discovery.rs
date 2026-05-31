//! Project instruction file discovery.
//!
//! Implements L3-BEH-CORE-008. Discovers AGENTS.md, AGENTS.override.md,
//! and configured fallback files along the directory hierarchy from
//! project root to cwd, plus global user-level instructions.

use std::path::{Path, PathBuf};

/// Configuration for instruction file discovery.
#[derive(Debug, Clone)]
pub struct InstructionDiscoveryConfig {
    /// Markers used to detect project root (e.g. [".git"]).
    pub root_markers: Vec<String>,
    /// Fallback filenames checked after AGENTS.md (e.g. ["CLAUDE.md", "PROMPT.md"]).
    pub fallback_filenames: Vec<String>,
    /// Maximum bytes per instruction file.
    pub max_file_bytes: usize,
    /// Maximum total bytes across all instruction files.
    pub max_total_bytes: usize,
}

impl Default for InstructionDiscoveryConfig {
    fn default() -> Self {
        Self {
            root_markers: vec![".git".into()],
            fallback_filenames: vec!["CLAUDE.md".into(), "PROMPT.md".into()],
            max_file_bytes: 65536,
            max_total_bytes: 262144,
        }
    }
}

/// The result of instruction file discovery.
#[derive(Debug, Clone)]
pub struct InstructionDiscovery {
    /// Absolute project root path, if found.
    pub project_root: Option<PathBuf>,
    /// Ancestor chain from root to cwd (inclusive).
    pub ancestor_chain: Vec<PathBuf>,
    /// Discovered instruction files in root-to-cwd order.
    pub discovered_files: Vec<InstructionFile>,
    /// Global instruction files from user config directory.
    pub global_files: Vec<InstructionFile>,
    /// Assembled instruction content.
    pub assembled_content: String,
    /// Total bytes of assembled content.
    pub total_bytes: usize,
}

/// A single discovered instruction file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// The filename that matched (AGENTS.md, AGENTS.override.md, or fallback).
    pub filename: String,
    /// Whether this is an override file (takes priority over all others in same dir).
    pub is_override: bool,
    /// Whether this came from the global config directory.
    pub is_global: bool,
    /// The file content.
    pub content: String,
    /// Content size in bytes.
    pub byte_count: usize,
}

/// Discovers the project root by walking up from cwd.
pub fn discover_project_root(cwd: &Path, markers: &[String]) -> Option<PathBuf> {
    let canonical_cwd = cwd.canonicalize().ok()?;
    for ancestor in canonical_cwd.ancestors() {
        for marker in markers {
            if ancestor.join(marker).exists() {
                return Some(ancestor.to_path_buf());
            }
        }
    }
    None
}

/// Builds the ancestor chain from project root to cwd (inclusive).
pub fn build_ancestor_chain(project_root: Option<&Path>, cwd: &Path) -> Vec<PathBuf> {
    let canonical_cwd = match cwd.canonicalize() {
        Ok(p) => p,
        Err(_) => return vec![cwd.to_path_buf()],
    };

    let root = match project_root {
        Some(r) => match r.canonicalize() {
            Ok(p) => p,
            Err(_) => return vec![canonical_cwd],
        },
        None => return vec![canonical_cwd],
    };

    let mut chain = Vec::new();
    for ancestor in canonical_cwd.ancestors() {
        chain.push(ancestor.to_path_buf());
        if ancestor == root {
            break;
        }
    }
    chain.reverse(); // root-to-cwd order
    chain
}

/// Resolve the global instruction directory for the current platform.
pub fn global_instruction_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(PathBuf::from(home).join(".devo"))
}

/// Discover instruction files in a single directory.
pub fn discover_in_directory(
    dir: &Path,
    config: &InstructionDiscoveryConfig,
) -> Vec<InstructionFile> {
    let mut files = Vec::new();

    // Priority 1: AGENTS.override.md
    let override_path = dir.join("AGENTS.override.md");
    if let Some(file) =
        read_instruction_file(&override_path, "AGENTS.override.md", true, false, config)
    {
        files.push(file);
        return files;
    }

    // Priority 2: AGENTS.md
    let agents_path = dir.join("AGENTS.md");
    if let Some(file) = read_instruction_file(&agents_path, "AGENTS.md", false, false, config) {
        files.push(file);
        return files;
    }

    // Priority 3: Fallback filenames
    for fallback in &config.fallback_filenames {
        let fallback_path = dir.join(fallback);
        if let Some(file) = read_instruction_file(&fallback_path, fallback, false, false, config) {
            files.push(file);
            return files;
        }
    }

    files
}

fn read_instruction_file(
    path: &Path,
    filename: &str,
    is_override: bool,
    is_global: bool,
    config: &InstructionDiscoveryConfig,
) -> Option<InstructionFile> {
    if !path.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    if content.len() > config.max_file_bytes {
        return None;
    }
    Some(InstructionFile {
        path: path.to_path_buf(),
        filename: filename.to_string(),
        is_override,
        is_global,
        content: trimmed.to_string(),
        byte_count: trimmed.len(),
    })
}

/// Discover all instruction files for a workspace.
pub fn discover_instructions(
    cwd: &Path,
    config: &InstructionDiscoveryConfig,
) -> InstructionDiscovery {
    let project_root = discover_project_root(cwd, &config.root_markers);
    let ancestor_chain = build_ancestor_chain(project_root.as_deref(), cwd);

    let mut discovered_files = Vec::new();
    let mut total_bytes: usize = 0;

    for dir in &ancestor_chain {
        let dir_files = discover_in_directory(dir, config);
        for file in dir_files {
            if total_bytes + file.byte_count > config.max_total_bytes {
                break;
            }
            total_bytes += file.byte_count;
            discovered_files.push(file);
        }
    }

    // Global instructions
    let mut global_files = Vec::new();
    if let Some(global_dir) = global_instruction_dir() {
        for filename in &["AGENTS.override.md", "AGENTS.md"] {
            let path = global_dir.join(filename);
            if let Some(file) =
                read_instruction_file(&path, filename, filename.contains("override"), true, config)
            {
                if total_bytes + file.byte_count <= config.max_total_bytes {
                    total_bytes += file.byte_count;
                    global_files.push(file);
                }
                break; // Only first existing file at global level
            }
        }
    }

    let assembled_content: String = global_files
        .iter()
        .chain(discovered_files.iter())
        .map(|f| f.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");

    InstructionDiscovery {
        project_root,
        ancestor_chain,
        discovered_files,
        global_files,
        assembled_content,
        total_bytes,
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn default_config_has_git_marker() {
        let config = InstructionDiscoveryConfig::default();
        assert!(config.root_markers.contains(&".git".to_string()));
        assert!(config.fallback_filenames.contains(&"CLAUDE.md".to_string()));
    }

    #[test]
    fn discover_project_root_finds_git_dir() {
        let tmp = TempDir::new().expect("tempdir");
        let git_dir = tmp.path().join(".git");
        fs::create_dir(&git_dir).expect("create .git");
        let subdir = tmp.path().join("src");
        fs::create_dir(&subdir).expect("create src");

        let root = discover_project_root(&subdir, &[".git".into()]);
        assert!(root.is_some());
        assert_eq!(root.unwrap(), tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn discover_project_root_returns_none_without_marker() {
        let tmp = TempDir::new().expect("tempdir");
        let root = discover_project_root(tmp.path(), &[".git".into()]);
        assert!(root.is_none());
    }

    #[test]
    fn build_ancestor_chain_root_to_cwd() {
        let tmp = TempDir::new().expect("tempdir");
        let git_dir = tmp.path().join(".git");
        fs::create_dir(&git_dir).expect("create .git");
        let subdir = tmp.path().join("src").join("lib");
        fs::create_dir_all(&subdir).expect("create dirs");
        let project_root = tmp.path().canonicalize().unwrap();

        let chain = build_ancestor_chain(Some(&project_root), &subdir);
        assert!(!chain.is_empty());
        assert_eq!(chain[0], project_root);
        assert_eq!(chain.last().unwrap(), &subdir.canonicalize().unwrap());
    }

    #[test]
    fn discover_in_directory_override_priority() {
        let tmp = TempDir::new().expect("tempdir");
        // Create both AGENTS.md and AGENTS.override.md
        fs::write(tmp.path().join("AGENTS.md"), "base").expect("write");
        fs::write(tmp.path().join("AGENTS.override.md"), "override").expect("write");

        let config = InstructionDiscoveryConfig::default();
        let files = discover_in_directory(tmp.path(), &config);
        assert_eq!(files.len(), 1);
        assert!(files[0].is_override);
        assert_eq!(files[0].content, "override");
    }

    #[test]
    fn discover_in_directory_fallback_to_claude_md() {
        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("CLAUDE.md"), "claude content").expect("write");

        let config = InstructionDiscoveryConfig::default();
        let files = discover_in_directory(tmp.path(), &config);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "CLAUDE.md");
    }

    #[test]
    fn empty_directory_returns_no_files() {
        let tmp = TempDir::new().expect("tempdir");
        let config = InstructionDiscoveryConfig::default();
        let files = discover_in_directory(tmp.path(), &config);
        assert!(files.is_empty());
    }

    #[test]
    fn skip_empty_files() {
        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("AGENTS.md"), "   \n  ").expect("write");
        let config = InstructionDiscoveryConfig::default();
        let files = discover_in_directory(tmp.path(), &config);
        assert!(files.is_empty());
    }

    #[test]
    fn instruction_file_non_empty() {
        let file = InstructionFile {
            path: PathBuf::from("/tmp/AGENTS.md"),
            filename: "AGENTS.md".into(),
            is_override: false,
            is_global: false,
            content: "Hello".into(),
            byte_count: 5,
        };
        assert!(!file.content.is_empty());
        assert_eq!(file.byte_count, 5);
    }
}
