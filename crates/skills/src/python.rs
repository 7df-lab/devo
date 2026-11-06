//! Python execution metadata derived from an already selected skill identity.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A host-selected bundled package. Python executes this exact package instead
/// of independently scanning roots or resolving an ambient module with its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonSkill {
    pub skill_path: PathBuf,
    pub module: String,
    pub package_dir: PathBuf,
}

impl PythonSkill {
    /// Bundled Python skills use `src/<skill_name_with_underscores>/__init__.py`.
    /// Markdown-only skills remain valid catalog entries without an executable.
    pub fn from_skill(name: &str, skill_path: &Path) -> Option<Self> {
        let module = name.replace('-', "_");
        let mut chars = module.chars();
        if !chars
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
            || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return None;
        }
        let package_dir = skill_path.parent()?.join("src").join(&module);
        package_dir.join("__init__.py").is_file().then(|| Self {
            skill_path: skill_path.to_path_buf(),
            module,
            package_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn resolves_the_selected_identity_without_scanning_other_roots() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("agent-message/SKILL.md");
        let package = root.path().join("agent-message/src/agent_message");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("__init__.py"), "").unwrap();
        assert_eq!(
            PythonSkill::from_skill("agent-message", &skill),
            Some(PythonSkill {
                skill_path: skill.clone(),
                module: "agent_message".into(),
                package_dir: package,
            })
        );
        assert_eq!(PythonSkill::from_skill("../agent-message", &skill), None);
        assert_eq!(PythonSkill::from_skill("markdown-only", &skill), None);
    }
}
