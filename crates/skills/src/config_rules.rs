//! Skill enable/disable rule resolution for effective configuration.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::model::SkillMetadata;
use crate::model::canonicalize_for_identity;

/// One selector that can enable or disable a loaded skill.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SkillConfigRuleSelector {
    Name(String),
    Path(PathBuf),
}

/// One effective skill rule.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkillConfigRule {
    pub selector: SkillConfigRuleSelector,
    pub enabled: bool,
}

/// Ordered effective skill rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SkillConfigRules {
    pub entries: Vec<SkillConfigRule>,
}

impl SkillConfigRules {
    pub fn from_entries<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (Option<PathBuf>, Option<String>, bool)>,
    {
        let entries = entries.into_iter();
        let (lower, upper) = entries.size_hint();
        let mut rules = Vec::with_capacity(upper.unwrap_or(lower));
        for (path, name, enabled) in entries {
            let selector = match (path, name) {
                (Some(path), None) => {
                    SkillConfigRuleSelector::Path(canonicalize_for_identity(&path))
                }
                (None, Some(name)) => {
                    let name = name.trim();
                    if name.is_empty() {
                        continue;
                    }
                    SkillConfigRuleSelector::Name(name.to_string())
                }
                (Some(_), Some(_)) | (None, None) => continue,
            };
            rules.retain(|rule: &SkillConfigRule| rule.selector != selector);
            rules.push(SkillConfigRule { selector, enabled });
        }
        Self { entries: rules }
    }
}

pub fn resolve_disabled_skill_paths(
    skills: &[SkillMetadata],
    rules: &SkillConfigRules,
) -> HashSet<PathBuf> {
    let mut disabled_paths = HashSet::new();
    for rule in &rules.entries {
        match &rule.selector {
            SkillConfigRuleSelector::Path(path) => {
                if rule.enabled {
                    disabled_paths.remove(path);
                } else {
                    disabled_paths.insert(path.clone());
                }
            }
            SkillConfigRuleSelector::Name(name) => {
                for path in skills
                    .iter()
                    .filter(|skill| skill.name == *name)
                    .map(|skill| canonicalize_for_identity(&skill.path_to_skills_md))
                {
                    if rule.enabled {
                        disabled_paths.remove(&path);
                    } else {
                        disabled_paths.insert(path);
                    }
                }
            }
        }
    }
    disabled_paths
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn from_entries_trims_names_and_keeps_last_rule() {
        let rules = SkillConfigRules::from_entries([
            (None, Some(" docs ".to_string()), false),
            (None, Some("docs".to_string()), true),
            (None, Some("   ".to_string()), false),
            (
                Some(PathBuf::from("skills/docs/SKILL.md")),
                Some("docs".to_string()),
                false,
            ),
        ]);

        assert_eq!(
            rules,
            SkillConfigRules {
                entries: vec![SkillConfigRule {
                    selector: SkillConfigRuleSelector::Name("docs".to_string()),
                    enabled: true,
                }],
            }
        );
    }
}
