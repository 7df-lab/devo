use crate::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use tracing::debug;

/// Search file contents with a regular expression.
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern in file contents. Returns matching lines with \
         file path and line number. Optionally restrict to files matching a glob pattern."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (default: cwd)"
                },
                "glob": {
                    "type": "string",
                    "description": "Only search files matching this glob (e.g. \"*.rs\")"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive matching (default: false)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolOutput> {
        let pattern_str = input["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'pattern' field"))?;

        let case_insensitive = input["case_insensitive"].as_bool().unwrap_or(false);

        let re = {
            let mut builder = regex::RegexBuilder::new(pattern_str);
            builder.case_insensitive(case_insensitive);
            match builder.build() {
                Ok(r) => r,
                Err(e) => return Ok(ToolOutput::error(format!("invalid regex: {}", e))),
            }
        };

        let base = match input["path"].as_str() {
            Some(p) => {
                let pb = PathBuf::from(p);
                if pb.is_absolute() { pb } else { ctx.cwd.join(pb) }
            }
            None => ctx.cwd.clone(),
        };

        let glob_pattern = input["glob"].as_str();

        debug!(pattern = pattern_str, base = %base.display(), "grep search");

        let files = collect_files(&base, glob_pattern);

        let mut results: Vec<String> = Vec::new();
        const MAX_RESULTS: usize = 500;

        'outer: for file in &files {
            let content = match tokio::fs::read_to_string(file).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (lineno, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    results.push(format!(
                        "{}:{}:{}",
                        file.to_string_lossy(),
                        lineno + 1,
                        line
                    ));
                    if results.len() >= MAX_RESULTS {
                        results.push(format!("(truncated at {} matches)", MAX_RESULTS));
                        break 'outer;
                    }
                }
            }
        }

        if results.is_empty() {
            return Ok(ToolOutput::success("(no matches)"));
        }

        Ok(ToolOutput::success(results.join("\n")))
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

fn collect_files(base: &std::path::Path, glob_pattern: Option<&str>) -> Vec<PathBuf> {
    let pattern = match glob_pattern {
        Some(g) => base.join("**").join(g).to_string_lossy().to_string(),
        None => base.join("**").join("*").to_string_lossy().to_string(),
    };

    glob::glob(&pattern)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|p| p.is_file())
        .collect()
}
