//! Explicit skill mention selection and `SKILL.md` loading.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::mention_counts::build_skill_name_counts;
use crate::model::SkillLoadOutcome;
use crate::model::SkillMetadata;
use crate::model::canonicalize_for_identity;

const SKILL_PATH_PREFIX: &str = "skill://";
const SKILL_FILENAME: &str = "SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSelection {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Default)]
pub struct SkillInjections {
    pub items: Vec<SkillInjection>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInjection {
    pub name: String,
    pub path: String,
    pub contents: String,
}

pub fn build_skill_injections(
    mentioned_skills: &[SkillMetadata],
    loaded_skills: Option<&SkillLoadOutcome>,
) -> SkillInjections {
    if mentioned_skills.is_empty() {
        return SkillInjections::default();
    }

    let mut result = SkillInjections {
        items: Vec::with_capacity(mentioned_skills.len()),
        warnings: Vec::new(),
    };

    for skill in mentioned_skills {
        if loaded_skills.is_some_and(|outcome| !outcome.is_skill_enabled(skill)) {
            result
                .warnings
                .push(format!("Skill {} is disabled", skill.name));
            continue;
        }
        match fs::read_to_string(&skill.path_to_skills_md) {
            Ok(contents) => result.items.push(SkillInjection {
                name: skill.name.clone(),
                path: skill.path_to_skills_md.display().to_string(),
                contents,
            }),
            Err(error) => result.warnings.push(format!(
                "Failed to load skill {} at {}: {error}",
                skill.name,
                skill.path_to_skills_md.display()
            )),
        }
    }

    result
}

pub fn collect_explicit_skill_mentions(
    texts: &[String],
    structured: &[SkillSelection],
    outcome: &SkillLoadOutcome,
) -> Vec<SkillMetadata> {
    let (skill_name_counts, _) = build_skill_name_counts(&outcome.skills, &outcome.disabled_paths);
    let mut selected = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut blocked_plain_names = HashSet::new();

    for selection in structured {
        blocked_plain_names.insert(selection.name.clone());
        let selection_path = canonicalize_for_identity(&selection.path);
        if outcome.disabled_paths.contains(&selection_path) || seen_paths.contains(&selection_path)
        {
            continue;
        }
        if let Some(skill) = outcome
            .skills
            .iter()
            .find(|skill| skill.path_to_skills_md == selection_path)
        {
            seen_paths.insert(skill.path_to_skills_md.clone());
            selected.push(skill.clone());
        }
    }

    for text in texts {
        let mentions = extract_tool_mentions(text);
        let mention_skill_paths = mentions
            .paths
            .iter()
            .filter(|path| is_skill_path(path))
            .map(|path| canonicalize_for_identity(Path::new(normalize_skill_path(path))))
            .collect::<HashSet<_>>();

        for skill in &outcome.skills {
            if outcome.disabled_paths.contains(&skill.path_to_skills_md)
                || seen_paths.contains(&skill.path_to_skills_md)
            {
                continue;
            }
            if mention_skill_paths.contains(&skill.path_to_skills_md) {
                seen_paths.insert(skill.path_to_skills_md.clone());
                selected.push(skill.clone());
            }
        }

        for skill in &outcome.skills {
            if outcome.disabled_paths.contains(&skill.path_to_skills_md)
                || seen_paths.contains(&skill.path_to_skills_md)
                || blocked_plain_names.contains(&skill.name)
                || !mentions.plain_names.contains(&skill.name)
            {
                continue;
            }
            if skill_name_counts.get(&skill.name).copied().unwrap_or(0) == 1 {
                seen_paths.insert(skill.path_to_skills_md.clone());
                selected.push(skill.clone());
            }
        }
    }

    selected
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ToolMentions {
    names: HashSet<String>,
    paths: HashSet<String>,
    plain_names: HashSet<String>,
}

pub fn extract_tool_mentions(text: &str) -> ToolMentions {
    let bytes = text.as_bytes();
    let mut mentions = ToolMentions::default();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'['
            && let Some((name, path, end_index)) = parse_linked_tool_mention(text, bytes, index)
        {
            if !is_common_env_var(name) {
                mentions.names.insert(name.to_string());
                mentions.paths.insert(path.to_string());
            }
            index = end_index;
            continue;
        }
        if byte != b'$' {
            index += 1;
            continue;
        }
        let name_start = index + 1;
        let Some(first_name_byte) = bytes.get(name_start) else {
            index += 1;
            continue;
        };
        if !is_mention_name_char(*first_name_byte) {
            index += 1;
            continue;
        }
        let mut name_end = name_start + 1;
        while let Some(next_byte) = bytes.get(name_end)
            && is_mention_name_char(*next_byte)
        {
            name_end += 1;
        }
        let name = &text[name_start..name_end];
        if !is_common_env_var(name) {
            mentions.names.insert(name.to_string());
            mentions.plain_names.insert(name.to_string());
        }
        index = name_end;
    }
    mentions
}

fn parse_linked_tool_mention<'a>(
    text: &'a str,
    bytes: &[u8],
    start: usize,
) -> Option<(&'a str, &'a str, usize)> {
    let sigil_index = start + 1;
    if bytes.get(sigil_index) != Some(&b'$') {
        return None;
    }
    let name_start = sigil_index + 1;
    let first_name_byte = bytes.get(name_start)?;
    if !is_mention_name_char(*first_name_byte) {
        return None;
    }
    let mut name_end = name_start + 1;
    while let Some(next_byte) = bytes.get(name_end)
        && is_mention_name_char(*next_byte)
    {
        name_end += 1;
    }
    if bytes.get(name_end) != Some(&b']') {
        return None;
    }
    let mut path_start = name_end + 1;
    while let Some(next_byte) = bytes.get(path_start)
        && next_byte.is_ascii_whitespace()
    {
        path_start += 1;
    }
    if bytes.get(path_start) != Some(&b'(') {
        return None;
    }
    let mut path_end = path_start + 1;
    while let Some(next_byte) = bytes.get(path_end)
        && *next_byte != b')'
    {
        path_end += 1;
    }
    if bytes.get(path_end) != Some(&b')') {
        return None;
    }
    let path = text[path_start + 1..path_end].trim();
    if path.is_empty() {
        return None;
    }
    let name = &text[name_start..name_end];
    Some((name, path, path_end + 1))
}

pub fn is_skill_path(path: &str) -> bool {
    path.starts_with(SKILL_PATH_PREFIX) || is_skill_filename(path)
}

pub fn normalize_skill_path(path: &str) -> &str {
    path.strip_prefix(SKILL_PATH_PREFIX).unwrap_or(path)
}

fn is_skill_filename(path: &str) -> bool {
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file_name.eq_ignore_ascii_case(SKILL_FILENAME)
}

fn is_mention_name_char(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b':')
}

fn is_common_env_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "PATH"
            | "HOME"
            | "USER"
            | "SHELL"
            | "PWD"
            | "TMPDIR"
            | "TEMP"
            | "TMP"
            | "LANG"
            | "TERM"
            | "XDG_CONFIG_HOME"
    )
}
