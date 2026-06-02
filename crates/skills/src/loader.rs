//! Filesystem skill discovery and `SKILL.md` metadata parsing.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;

use crate::config_rules::SkillConfigRules;
use crate::config_rules::resolve_disabled_skill_paths;
use crate::model::SkillDependencies;
use crate::model::SkillError;
use crate::model::SkillInterface;
use crate::model::SkillLoadOutcome;
use crate::model::SkillMetadata;
use crate::model::SkillPolicy;
use crate::model::SkillScope;
use crate::model::SkillToolDependency;
use crate::model::canonicalize_for_identity;

const SKILLS_FILENAME: &str = "SKILL.md";
const SKILLS_METADATA_DIR: &str = "agents";
const SKILLS_METADATA_FILENAME: &str = "openai.yaml";
const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 1024;
const MAX_SHORT_DESCRIPTION_LEN: usize = MAX_DESCRIPTION_LEN;
const MAX_DEFAULT_PROMPT_LEN: usize = MAX_DESCRIPTION_LEN;
const MAX_DEPENDENCY_TYPE_LEN: usize = MAX_NAME_LEN;
const MAX_DEPENDENCY_TRANSPORT_LEN: usize = MAX_NAME_LEN;
const MAX_DEPENDENCY_VALUE_LEN: usize = MAX_DESCRIPTION_LEN;
const MAX_DEPENDENCY_DESCRIPTION_LEN: usize = MAX_DESCRIPTION_LEN;
const MAX_DEPENDENCY_COMMAND_LEN: usize = MAX_DESCRIPTION_LEN;
const MAX_DEPENDENCY_URL_LEN: usize = MAX_DESCRIPTION_LEN;
const MAX_SCAN_DEPTH: usize = 6;
const MAX_SKILLS_DIRS_PER_ROOT: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    pub path: PathBuf,
    pub scope: SkillScope,
    pub plugin_id: Option<String>,
}

pub fn load_skills_from_roots<I>(roots: I, rules: &SkillConfigRules) -> SkillLoadOutcome
where
    I: IntoIterator<Item = SkillRoot>,
{
    let mut outcome = SkillLoadOutcome::default();
    let mut skill_roots = Vec::new();
    let mut skill_root_by_path = HashMap::new();

    for root in roots {
        let root_path = canonicalize_for_identity(&root.path);
        let skills_before_root = outcome.skills.len();
        discover_skills_under_root(&root, &root_path, &mut outcome);
        for skill in &outcome.skills[skills_before_root..] {
            if !skill_roots.contains(&root_path) {
                skill_roots.push(root_path.clone());
            }
            skill_root_by_path
                .entry(skill.path_to_skills_md.clone())
                .or_insert_with(|| root_path.clone());
        }
    }

    let mut seen_paths = HashSet::new();
    outcome
        .skills
        .retain(|skill| seen_paths.insert(skill.path_to_skills_md.clone()));
    let retained_skill_paths = outcome
        .skills
        .iter()
        .map(|skill| skill.path_to_skills_md.clone())
        .collect::<HashSet<_>>();
    skill_root_by_path.retain(|path, _| retained_skill_paths.contains(path));
    let used_roots = skill_root_by_path.values().cloned().collect::<HashSet<_>>();
    skill_roots.retain(|root| used_roots.contains(root));
    outcome.skill_roots = skill_roots;
    outcome.skill_root_by_path = Arc::new(skill_root_by_path);
    outcome.skills.sort_by(|left, right| {
        scope_rank(left.scope)
            .cmp(&scope_rank(right.scope))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.path_to_skills_md.cmp(&right.path_to_skills_md))
    });
    outcome.disabled_paths = resolve_disabled_skill_paths(&outcome.skills, rules);
    build_implicit_indexes(&mut outcome);
    outcome
}

fn scope_rank(scope: SkillScope) -> u8 {
    match scope {
        SkillScope::Repo => 0,
        SkillScope::User => 1,
        SkillScope::System => 2,
        SkillScope::Admin => 3,
        SkillScope::Plugin => 4,
    }
}

fn discover_skills_under_root(root: &SkillRoot, root_path: &Path, outcome: &mut SkillLoadOutcome) {
    match fs::metadata(root_path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            outcome.errors.push(SkillError {
                path: root_path.to_path_buf(),
                message: format!("failed to stat skills root: {error}"),
            });
            return;
        }
    }

    let mut visited_dirs = HashSet::new();
    visited_dirs.insert(root_path.to_path_buf());
    let mut queue = VecDeque::from([(root_path.to_path_buf(), 0usize)]);
    let mut truncated_by_dir_limit = false;

    while let Some((dir, depth)) = queue.pop_front() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                outcome.errors.push(SkillError {
                    path: dir,
                    message: format!("failed to read skills dir: {error}"),
                });
                continue;
            }
        };

        for entry in entries.flatten() {
            let file_name = entry.file_name();
            if file_name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    outcome.errors.push(SkillError {
                        path,
                        message: format!("failed to stat skills path: {error}"),
                    });
                    continue;
                }
            };
            if metadata.is_dir() {
                enqueue_dir(
                    &mut queue,
                    &mut visited_dirs,
                    &mut truncated_by_dir_limit,
                    path,
                    depth + 1,
                );
                continue;
            }
            if metadata.is_file() && file_name == SKILLS_FILENAME {
                match parse_skill_file(&path, root.scope, root.plugin_id.as_deref()) {
                    Ok(skill) => outcome.skills.push(skill),
                    Err(message) => {
                        if root.scope != SkillScope::System {
                            outcome.errors.push(SkillError { path, message });
                        }
                    }
                }
            }
        }
    }

    if truncated_by_dir_limit {
        outcome.errors.push(SkillError {
            path: root_path.to_path_buf(),
            message: format!("skills scan truncated after {MAX_SKILLS_DIRS_PER_ROOT} directories"),
        });
    }
}

fn enqueue_dir(
    queue: &mut VecDeque<(PathBuf, usize)>,
    visited_dirs: &mut HashSet<PathBuf>,
    truncated_by_dir_limit: &mut bool,
    path: PathBuf,
    depth: usize,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    if visited_dirs.len() >= MAX_SKILLS_DIRS_PER_ROOT {
        *truncated_by_dir_limit = true;
        return;
    }
    let path = canonicalize_for_identity(&path);
    if visited_dirs.insert(path.clone()) {
        queue.push_back((path, depth));
    }
}

fn parse_skill_file(
    path: &Path,
    scope: SkillScope,
    plugin_id: Option<&str>,
) -> Result<SkillMetadata, String> {
    let contents =
        fs::read_to_string(path).map_err(|error| format!("failed to read file: {error}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "SKILL.md has no parent directory".to_string())?;
    let fallback_name = parent
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown-skill")
        .to_string();
    let (frontmatter, _body) = parse_frontmatter(path, &contents, &fallback_name)?;
    let metadata = load_metadata_file(parent)?;

    let canonical_path = canonicalize_for_identity(path);
    Ok(SkillMetadata {
        name: frontmatter.name,
        description: frontmatter.description,
        short_description: frontmatter.short_description,
        interface: metadata.interface,
        dependencies: metadata.dependencies,
        policy: metadata.policy,
        path_to_skills_md: canonical_path,
        scope,
        plugin_id: plugin_id.map(ToOwned::to_owned),
    })
}

fn parse_frontmatter(
    path: &Path,
    contents: &str,
    fallback_name: &str,
) -> Result<(ParsedFrontmatter, String), String> {
    let Some(stripped) = contents.strip_prefix("---") else {
        return Ok((
            ParsedFrontmatter {
                name: validate_name(fallback_name)?,
                description: format!("Skill discovered at {}", path.display()),
                short_description: None,
            },
            contents.to_string(),
        ));
    };
    let Some(end_index) = stripped.find("\n---") else {
        return Err("missing YAML frontmatter terminator".to_string());
    };
    let raw = stripped[..end_index].trim();
    let body = stripped[end_index + 4..]
        .trim_start_matches(['\r', '\n'])
        .to_string();
    let frontmatter: SkillFrontmatter =
        serde_yaml::from_str(raw).map_err(|error| format!("invalid YAML: {error}"))?;
    let name = validate_name(
        frontmatter
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(fallback_name),
    )?;
    let description = frontmatter
        .description
        .filter(|description| !description.trim().is_empty())
        .unwrap_or_else(|| format!("Skill discovered at {}", path.display()));
    validate_len("description", &description, MAX_DESCRIPTION_LEN)?;
    if let Some(short_description) = frontmatter.metadata.short_description.as_deref() {
        validate_len(
            "metadata.short-description",
            short_description,
            MAX_SHORT_DESCRIPTION_LEN,
        )?;
    }
    Ok((
        ParsedFrontmatter {
            name,
            description,
            short_description: frontmatter.metadata.short_description,
        },
        body,
    ))
}

fn validate_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("missing field `name`".to_string());
    }
    validate_len("name", trimmed, MAX_NAME_LEN)?;
    Ok(trimmed.to_string())
}

fn validate_len(field: &'static str, value: &str, max_len: usize) -> Result<(), String> {
    if value.chars().count() > max_len {
        Err(format!("invalid {field}: exceeds {max_len} characters"))
    } else {
        Ok(())
    }
}

fn load_metadata_file(package_root: &Path) -> Result<LoadedSkillMetadata, String> {
    let metadata_path = package_root
        .join(SKILLS_METADATA_DIR)
        .join(SKILLS_METADATA_FILENAME);
    if !metadata_path.exists() {
        return Ok(LoadedSkillMetadata::default());
    }
    let raw = fs::read_to_string(&metadata_path)
        .map_err(|error| format!("failed to read agents/openai.yaml: {error}"))?;
    let metadata: SkillMetadataFile = serde_yaml::from_str(&raw)
        .map_err(|error| format!("invalid agents/openai.yaml: {error}"))?;
    metadata.try_into()
}

fn build_implicit_indexes(outcome: &mut SkillLoadOutcome) {
    let mut by_scripts_dir = HashMap::new();
    let mut by_doc_path = HashMap::new();
    for skill in &outcome.skills {
        let Some(package_root) = skill.path_to_skills_md.parent() else {
            continue;
        };
        let scripts_dir = package_root.join("scripts");
        if scripts_dir.is_dir() {
            by_scripts_dir.insert(canonicalize_for_identity(&scripts_dir), skill.clone());
        }
        by_doc_path.insert(skill.path_to_skills_md.clone(), skill.clone());
    }
    outcome.implicit_skills_by_scripts_dir = Arc::new(by_scripts_dir);
    outcome.implicit_skills_by_doc_path = Arc::new(by_doc_path);
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    metadata: SkillFrontmatterMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatterMetadata {
    #[serde(default, rename = "short-description")]
    short_description: Option<String>,
}

#[derive(Debug)]
struct ParsedFrontmatter {
    name: String,
    description: String,
    short_description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SkillMetadataFile {
    #[serde(default)]
    interface: Option<Interface>,
    #[serde(default)]
    dependencies: Option<Dependencies>,
    #[serde(default)]
    policy: Option<Policy>,
}

#[derive(Default)]
struct LoadedSkillMetadata {
    interface: Option<SkillInterface>,
    dependencies: Option<SkillDependencies>,
    policy: Option<SkillPolicy>,
}

impl TryFrom<SkillMetadataFile> for LoadedSkillMetadata {
    type Error = String;

    fn try_from(value: SkillMetadataFile) -> Result<Self, Self::Error> {
        Ok(Self {
            interface: value.interface.map(TryInto::try_into).transpose()?,
            dependencies: value.dependencies.map(TryInto::try_into).transpose()?,
            policy: value.policy.map(Into::into),
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct Interface {
    display_name: Option<String>,
    short_description: Option<String>,
    icon_small: Option<PathBuf>,
    icon_large: Option<PathBuf>,
    brand_color: Option<String>,
    default_prompt: Option<String>,
}

impl TryFrom<Interface> for SkillInterface {
    type Error = String;

    fn try_from(value: Interface) -> Result<Self, Self::Error> {
        if let Some(short_description) = value.short_description.as_deref() {
            validate_len(
                "interface.short_description",
                short_description,
                MAX_SHORT_DESCRIPTION_LEN,
            )?;
        }
        if let Some(default_prompt) = value.default_prompt.as_deref() {
            validate_len(
                "interface.default_prompt",
                default_prompt,
                MAX_DEFAULT_PROMPT_LEN,
            )?;
        }
        Ok(Self {
            display_name: value.display_name,
            short_description: value.short_description,
            icon_small: value.icon_small,
            icon_large: value.icon_large,
            brand_color: value.brand_color,
            default_prompt: value.default_prompt,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct Dependencies {
    #[serde(default)]
    tools: Vec<DependencyTool>,
}

impl TryFrom<Dependencies> for SkillDependencies {
    type Error = String;

    fn try_from(value: Dependencies) -> Result<Self, Self::Error> {
        let mut tools = Vec::new();
        for tool in value.tools {
            tools.push(tool.try_into()?);
        }
        Ok(Self { tools })
    }
}

#[derive(Debug, Default, Deserialize)]
struct DependencyTool {
    #[serde(rename = "type")]
    kind: Option<String>,
    value: Option<String>,
    description: Option<String>,
    transport: Option<String>,
    command: Option<String>,
    url: Option<String>,
}

impl TryFrom<DependencyTool> for SkillToolDependency {
    type Error = String;

    fn try_from(value: DependencyTool) -> Result<Self, Self::Error> {
        let kind = required_limited(value.kind, "dependency.type", MAX_DEPENDENCY_TYPE_LEN)?;
        let dependency_value =
            required_limited(value.value, "dependency.value", MAX_DEPENDENCY_VALUE_LEN)?;
        validate_optional(
            "dependency.description",
            &value.description,
            MAX_DEPENDENCY_DESCRIPTION_LEN,
        )?;
        validate_optional(
            "dependency.transport",
            &value.transport,
            MAX_DEPENDENCY_TRANSPORT_LEN,
        )?;
        validate_optional(
            "dependency.command",
            &value.command,
            MAX_DEPENDENCY_COMMAND_LEN,
        )?;
        validate_optional("dependency.url", &value.url, MAX_DEPENDENCY_URL_LEN)?;
        Ok(Self {
            r#type: kind,
            value: dependency_value,
            description: value.description,
            transport: value.transport,
            command: value.command,
            url: value.url,
        })
    }
}

fn required_limited(
    value: Option<String>,
    field: &'static str,
    max_len: usize,
) -> Result<String, String> {
    let value = value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing field `{field}`"))?;
    validate_len(field, &value, max_len)?;
    Ok(value)
}

fn validate_optional(
    field: &'static str,
    value: &Option<String>,
    max_len: usize,
) -> Result<(), String> {
    if let Some(value) = value {
        validate_len(field, value, max_len)?;
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
struct Policy {
    #[serde(default)]
    allow_implicit_invocation: Option<bool>,
    #[serde(default)]
    products: Vec<String>,
}

impl From<Policy> for SkillPolicy {
    fn from(value: Policy) -> Self {
        Self {
            allow_implicit_invocation: value.allow_implicit_invocation,
            products: value.products,
        }
    }
}
