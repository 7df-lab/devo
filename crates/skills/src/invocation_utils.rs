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
        if workdir.starts_with(scripts_dir) || command.contains(&scripts_dir.display().to_string())
        {
            return Some(skill.clone());
        }
    }
    None
}
