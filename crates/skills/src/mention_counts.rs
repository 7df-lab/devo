//! Helpers for detecting ambiguous skill names.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::model::SkillMetadata;
use crate::model::canonicalize_for_identity;

pub fn build_skill_name_counts(
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<PathBuf>,
) -> (HashMap<String, usize>, HashMap<String, usize>) {
    let mut exact = HashMap::new();
    let mut lowercase = HashMap::new();
    for skill in skills {
        let path = canonicalize_for_identity(&skill.path_to_skills_md);
        if disabled_paths.contains(&path) {
            continue;
        }
        *exact.entry(skill.name.clone()).or_insert(0) += 1;
        *lowercase
            .entry(skill.name.to_ascii_lowercase())
            .or_insert(0) += 1;
    }
    (exact, lowercase)
}
