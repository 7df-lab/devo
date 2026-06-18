//! Helpers for detecting ambiguous skill names.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::model::SkillMetadata;
use crate::model::path_set_contains_identity;

pub fn build_skill_name_counts(
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<PathBuf>,
) -> (HashMap<String, usize>, HashMap<String, usize>) {
    let mut exact = HashMap::with_capacity(skills.len());
    let mut lowercase = HashMap::with_capacity(skills.len());
    for skill in skills {
        if path_set_contains_identity(disabled_paths, &skill.path_to_skills_md) {
            continue;
        }
        *exact.entry(skill.name.clone()).or_insert(0) += 1;
        *lowercase
            .entry(skill.name.to_ascii_lowercase())
            .or_insert(0) += 1;
    }
    (exact, lowercase)
}
