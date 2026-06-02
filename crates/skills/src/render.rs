//! Model-visible rendering for available skill metadata.

use crate::model::SkillLoadOutcome;
use crate::model::SkillMetadata;

const DEFAULT_SKILL_METADATA_CHAR_BUDGET: usize = 8_000;
const SKILL_METADATA_CONTEXT_WINDOW_PERCENT: usize = 2;
const APPROX_BYTES_PER_TOKEN: usize = 4;

pub const SKILL_DESCRIPTION_TRUNCATED_WARNING: &str = "Skill descriptions were shortened to fit the skills context budget. Devo can still see every skill, but some descriptions are shorter. Disable unused skills to leave more room for the rest.";
pub const SKILL_DESCRIPTIONS_REMOVED_WARNING_PREFIX: &str =
    "Exceeded skills context budget. All skill descriptions were removed and";
pub const SKILLS_INTRO_WITH_ABSOLUTE_PATHS: &str = "A skill is a set of local instructions to follow that is stored in a `SKILL.md` file. Below is the list of skills that can be used. Each entry includes a name, description, and file path so you can open the source for full instructions when using a specific skill.";
pub const SKILLS_HOW_TO_USE_WITH_ABSOLUTE_PATHS: &str = r###"- Discovery: The list above is the skills available in this session (name + description + file path). Skill bodies live on disk at the listed paths.
- Trigger rules: If the user names a skill (with `$SkillName` or plain text) OR the task clearly matches a skill's description shown above, you must use that skill for that turn. Multiple mentions mean use them all. Do not carry skills across turns unless re-mentioned.
- Missing/blocked: If a named skill isn't in the list or the path can't be read, say so briefly and continue with the best fallback.
- How to use a skill (progressive disclosure):
  1) After deciding to use a skill, open its `SKILL.md`. Read only enough to follow the workflow.
  2) When `SKILL.md` references relative paths (e.g., `scripts/foo.py`), resolve them relative to the skill directory listed above first, and only consider other paths if needed.
  3) If `SKILL.md` points to extra folders such as `references/`, load only the specific files needed for the request; don't bulk-load everything.
  4) If `scripts/` exist, prefer running or patching them instead of retyping large code blocks.
  5) If `assets/` or templates exist, reuse them instead of recreating from scratch.
- Coordination and sequencing:
  - If multiple skills apply, choose the minimal set that covers the request and state the order you'll use them.
  - Announce which skill(s) you're using and why (one short line). If you skip an obvious skill, say why.
- Context hygiene:
  - Keep context small: summarize long sections instead of pasting them; only load extra files when needed.
  - Avoid deep reference-chasing: prefer opening only files directly linked from `SKILL.md` unless you're blocked.
  - When variants exist (frameworks, providers, domains), pick only the relevant reference file(s) and note that choice.
- Safety and fallback: If a skill can't be applied cleanly (missing files, unclear instructions), state the issue, pick the next-best approach, and continue."###;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillMetadataBudget {
    Tokens(usize),
    Characters(usize),
}

impl SkillMetadataBudget {
    fn cost(self, text: &str) -> usize {
        match self {
            Self::Tokens(_) => approx_token_count(text),
            Self::Characters(_) => text.chars().count(),
        }
    }

    fn limit(self) -> usize {
        match self {
            Self::Tokens(limit) | Self::Characters(limit) => limit,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRenderReport {
    pub total_count: usize,
    pub included_count: usize,
    pub omitted_count: usize,
    pub truncated_description_chars: usize,
    pub truncated_description_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableSkills {
    pub skill_root_lines: Vec<String>,
    pub skill_lines: Vec<String>,
    pub report: SkillRenderReport,
    pub warning_message: Option<String>,
}

pub fn default_skill_metadata_budget(context_window: Option<i64>) -> SkillMetadataBudget {
    context_window
        .and_then(|window| usize::try_from(window).ok())
        .filter(|window| *window > 0)
        .map(|window| {
            SkillMetadataBudget::Tokens(
                window
                    .saturating_mul(SKILL_METADATA_CONTEXT_WINDOW_PERCENT)
                    .saturating_div(100)
                    .max(1),
            )
        })
        .unwrap_or(SkillMetadataBudget::Characters(
            DEFAULT_SKILL_METADATA_CHAR_BUDGET,
        ))
}

pub fn build_available_skills(
    outcome: &SkillLoadOutcome,
    budget: SkillMetadataBudget,
) -> Option<AvailableSkills> {
    let skills = outcome.allowed_skills_for_implicit_invocation();
    if skills.is_empty() {
        return None;
    }

    let mut skill_lines = Vec::new();
    let mut included_count = 0;
    let mut omitted_count = 0;
    let mut truncated_description_chars = 0;
    let mut truncated_description_count = 0;
    let mut used = 0usize;

    for skill in &skills {
        let full_line = skill_line(skill, Some(skill_description(skill)));
        let cost = budget.cost(&full_line);
        if used.saturating_add(cost) <= budget.limit() {
            used = used.saturating_add(cost);
            included_count += 1;
            skill_lines.push(full_line);
            continue;
        }

        let no_description = skill_line(skill, None);
        let no_description_cost = budget.cost(&no_description);
        if used.saturating_add(no_description_cost) <= budget.limit() {
            truncated_description_chars += skill_description(skill).chars().count();
            truncated_description_count += 1;
            included_count += 1;
            used = used.saturating_add(no_description_cost);
            skill_lines.push(no_description);
        } else {
            omitted_count += 1;
        }
    }

    let warning_message = if omitted_count > 0 {
        let skill_word = if omitted_count == 1 {
            "skill"
        } else {
            "skills"
        };
        let verb = if omitted_count == 1 { "was" } else { "were" };
        Some(format!(
            "{SKILL_DESCRIPTIONS_REMOVED_WARNING_PREFIX} {omitted_count} additional {skill_word} {verb} not included in the model-visible skills list."
        ))
    } else if truncated_description_count > 0 {
        Some(SKILL_DESCRIPTION_TRUNCATED_WARNING.to_string())
    } else {
        None
    };

    Some(AvailableSkills {
        skill_root_lines: Vec::new(),
        skill_lines,
        report: SkillRenderReport {
            total_count: skills.len(),
            included_count,
            omitted_count,
            truncated_description_chars,
            truncated_description_count,
        },
        warning_message,
    })
}

pub fn render_available_skills_body(skill_root_lines: &[String], skill_lines: &[String]) -> String {
    let mut lines = Vec::new();
    lines.push("## Skills".to_string());
    lines.push(SKILLS_INTRO_WITH_ABSOLUTE_PATHS.to_string());
    if !skill_root_lines.is_empty() {
        lines.push("### Skill roots".to_string());
        lines.extend(skill_root_lines.iter().cloned());
    }
    lines.push("### Available skills".to_string());
    lines.extend(skill_lines.iter().cloned());
    lines.push("### How to use skills".to_string());
    lines.push(SKILLS_HOW_TO_USE_WITH_ABSOLUTE_PATHS.to_string());
    format!("\n{}\n", lines.join("\n"))
}

fn skill_line(skill: &SkillMetadata, description: Option<&str>) -> String {
    match description.filter(|description| !description.trim().is_empty()) {
        Some(description) => format!(
            "- {}: {} (path: {})",
            skill.name,
            description.trim(),
            skill.path_to_skills_md.display()
        ),
        None => format!(
            "- {} (path: {})",
            skill.name,
            skill.path_to_skills_md.display()
        ),
    }
}

fn skill_description(skill: &SkillMetadata) -> &str {
    skill
        .interface
        .as_ref()
        .and_then(|interface| interface.short_description.as_deref())
        .or(skill.short_description.as_deref())
        .unwrap_or(&skill.description)
}

fn approx_token_count(text: &str) -> usize {
    text.len()
        .saturating_add(APPROX_BYTES_PER_TOKEN.saturating_sub(1))
        / APPROX_BYTES_PER_TOKEN
}
