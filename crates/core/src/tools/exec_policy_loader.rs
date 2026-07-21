//! Load exec policy rule files from the Devo home directory.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use devo_execpolicy::{Error as ExecPolicyRuleError, Policy, PolicyParser};
use devo_util_paths::find_devo_home;
use thiserror::Error;

const RULES_DIR_NAME: &str = "rules";
const RULE_EXTENSION: &str = "rules";

#[derive(Debug, Error)]
pub enum ExecPolicyLoadError {
    #[error("failed to resolve DEVO_HOME: {0}")]
    DevoHome(#[from] std::io::Error),

    #[error("failed to read rules directory {dir}: {source}")]
    ReadDir {
        dir: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to read rules file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse rules file {path}: {source}")]
    ParsePolicy {
        path: String,
        source: Box<ExecPolicyRuleError>,
    },
}

/// Loads all `*.rules` files from `$DEVO_HOME/rules/` (or `~/.devo/rules/`).
pub fn load_exec_policy_from_devo_home() -> Result<Policy, ExecPolicyLoadError> {
    let devo_home = find_devo_home()?;
    load_exec_policy_from_dir(devo_home.join(RULES_DIR_NAME))
}

/// Loads all `*.rules` files from `rules_dir`, sorted by path.
pub fn load_exec_policy_from_dir(
    rules_dir: impl AsRef<Path>,
) -> Result<Policy, ExecPolicyLoadError> {
    let policy_paths = collect_policy_files(rules_dir.as_ref())?;
    let mut parser = PolicyParser::new();
    for policy_path in &policy_paths {
        let contents = std::fs::read_to_string(policy_path).map_err(|source| {
            ExecPolicyLoadError::ReadFile {
                path: policy_path.clone(),
                source,
            }
        })?;
        let identifier = policy_path.to_string_lossy().to_string();
        parser.parse(&identifier, &contents).map_err(|source| {
            ExecPolicyLoadError::ParsePolicy {
                path: identifier,
                source: Box::new(source),
            }
        })?;
    }
    Ok(parser.build())
}

fn collect_policy_files(dir: &Path) -> Result<Vec<PathBuf>, ExecPolicyLoadError> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ExecPolicyLoadError::ReadDir {
                dir: dir.to_path_buf(),
                source,
            });
        }
    };

    let mut policy_paths = read_dir
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let file_type = entry.file_type().ok()?;
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == RULE_EXTENSION)
                && file_type.is_file()
            {
                Some(path)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    policy_paths.sort();
    Ok(policy_paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_execpolicy::Decision;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn loads_rules_from_directory() {
        let tmp = tempdir().expect("create temp dir");
        let rules_dir = tmp.path().join("rules");
        std::fs::create_dir_all(&rules_dir).expect("create rules dir");
        std::fs::write(
            rules_dir.join("allow_git.rules"),
            r#"prefix_rule(pattern=["git", "status"], decision="allow")
"#,
        )
        .expect("write rules file");

        let policy = load_exec_policy_from_dir(&rules_dir).expect("load policy");
        let fallback = |_cmd: &[String]| Decision::Prompt;
        let evaluation = policy.check(&["git".to_string(), "status".to_string()], &fallback);
        assert_eq!(evaluation.decision, Decision::Allow);
    }

    #[test]
    fn missing_rules_directory_returns_empty_policy() {
        let tmp = tempdir().expect("create temp dir");
        let policy = load_exec_policy_from_dir(tmp.path().join("missing-rules"))
            .expect("missing rules dir should yield empty policy");
        let fallback = |_cmd: &[String]| Decision::Prompt;
        let evaluation = policy.check(&["git".to_string(), "status".to_string()], &fallback);
        // Empty policy falls through to heuristics fallback.
        assert_eq!(evaluation.decision, Decision::Prompt);
    }
}
