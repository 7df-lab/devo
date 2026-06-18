use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};

const SKILL_DESCRIPTION: &str = include_str!("../skill.txt");

pub struct SkillHandler {
    spec: ToolSpec,
}

impl Default for SkillHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "skill".into(),
                description: SKILL_DESCRIPTION.into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([(
                        "name".to_string(),
                        JsonSchema::string(Some("The name of the skill from available_skills")),
                    )]),
                    Some(vec!["name".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![],
                supports_parallel: true,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for SkillHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let name = input["name"].as_str().unwrap_or("");

        let found = find_skill(&ctx.workspace_root, name)
            .ok_or_else(|| ToolCallError::ExecutionFailed(format!("Skill \"{name}\" not found")))?;

        let content = fs::read_to_string(&found)
            .await
            .map_err(|e| ToolCallError::ExecutionFailed(format!("Failed to read skill: {e}")))?;

        let dir = found.parent().unwrap_or(Path::new("")).to_path_buf();
        let file_list = sample_file_list(&dir);

        Ok(ToolResult::success(
            ToolResultContent::Text(format!(
                "<skill_content name=\"{name}\">\n# Skill: {name}\n\n{content}\n\nBase directory for this skill: {}\nRelative paths in this skill (e.g., scripts/, reference/) are relative to this base directory.\nNote: file list is sampled.\n\n<skill_files>\n{file_list}\n</skill_files>\n</skill_content>",
                dir.display(),
            )),
            "Skill loaded",
        ))
    }
}

fn find_skill(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(read) = std::fs::read_dir(&dir) {
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().and_then(|x| x.to_str()) == Some("SKILL.md")
                    && path.parent()?.file_name().and_then(|x| x.to_str()) == Some(name)
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn sample_file_list(dir: &Path) -> String {
    let mut files = String::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        let mut count = 0usize;
        for entry in read.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|x| x.to_str()) == Some("SKILL.md") {
                continue;
            }
            if count > 0 {
                files.push('\n');
            }
            let _ = write!(files, "<file>{}</file>", path.display());
            count += 1;
            if count >= 10 {
                break;
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn sample_file_list_excludes_skill_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("SKILL.md"), "skill").expect("write skill");
        let reference_path = dir.path().join("reference.md");
        std::fs::write(&reference_path, "reference").expect("write reference");

        assert_eq!(
            sample_file_list(dir.path()),
            format!("<file>{}</file>", reference_path.display())
        );
    }
}
