//! Implicit invocation helpers for skill-owned scripts and docs.

use std::path::Path;

use crate::model::SkillLoadOutcome;
use crate::model::SkillMetadata;
use crate::model::canonicalize_for_identity;

pub fn detect_implicit_skill_invocation_for_command(
    outcome: Option<&SkillLoadOutcome>,
    command: &str,
    workdir: &Path,
) -> Option<SkillMetadata> {
    let outcome = outcome?;
    let workdir = canonicalize_for_identity(workdir);
    for (scripts_dir, skill) in outcome.implicit_skills_by_scripts_dir.iter() {
        if workdir.starts_with(scripts_dir) || command_mentions_path(command, scripts_dir) {
            return Some(skill.clone());
        }
    }
    None
}

fn command_mentions_path(command: &str, path: &Path) -> bool {
    if let Some(path) = path.to_str() {
        command.contains(path)
    } else {
        command.contains(&path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;

    use pretty_assertions::assert_eq;

    use super::detect_implicit_skill_invocation_for_command;
    use crate::model::SkillLoadOutcome;
    use crate::model::SkillMetadata;
    use crate::model::SkillScope;

    fn skill(path: &str) -> SkillMetadata {
        SkillMetadata {
            name: "scripted".to_string(),
            description: "Runs scripts".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            path_to_skills_md: PathBuf::from(path).join("SKILL.md"),
            scope: SkillScope::Repo,
            plugin_id: None,
        }
    }

    #[test]
    fn detects_command_that_mentions_scripts_dir() {
        let scripts_dir = PathBuf::from("skills/scripted/scripts");
        let skill = skill("skills/scripted");
        let outcome = SkillLoadOutcome {
            implicit_skills_by_scripts_dir: Arc::new(HashMap::from([(
                scripts_dir.clone(),
                skill.clone(),
            )])),
            ..SkillLoadOutcome::default()
        };
        let command = format!("{} build", scripts_dir.display());

        assert_eq!(
            detect_implicit_skill_invocation_for_command(
                Some(&outcome),
                &command,
                Path::new("workspace"),
            ),
            Some(skill)
        );
    }
}
