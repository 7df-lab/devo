use std::ffi::OsString;

use async_trait::async_trait;
use tracing::debug;

use super::ripgrep::RG_NO_MATCH_EXIT_CODE;
use super::ripgrep::ripgrep_failure_message;
use super::ripgrep::run_rg;
use crate::contracts::{
    ToolCallError, ToolContext, ToolProgressSender, ToolResult, ToolResultContent,
};
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};

const GLOB_DESCRIPTION: &str = include_str!("../glob.txt");

pub struct GlobHandler {
    spec: ToolSpec,
}

impl Default for GlobHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "find".into(),
                description: GLOB_DESCRIPTION.into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "pattern".to_string(),
                            JsonSchema::string(Some(
                                "The ripgrep glob pattern to match file paths against",
                            )),
                        ),
                        (
                            "path".to_string(),
                            JsonSchema::string(Some(
                                "The directory to search in. Defaults to the workspace root.",
                            )),
                        ),
                    ]),
                    Some(vec!["pattern".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![crate::tool_spec::ToolCapabilityTag::SearchWorkspace],
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
impl ToolHandler for GlobHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let pattern = input["pattern"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'pattern' field".into()))?;

        let path = input["path"].as_str().unwrap_or(".");
        debug!(pattern, path, "find search");

        let output = run_rg(
            &ctx,
            [
                OsString::from("--files"),
                OsString::from("--glob"),
                OsString::from(pattern),
                OsString::from("--"),
                OsString::from(path),
            ],
        )
        .await?;

        let exit_code = output.status.code().unwrap_or(i32::MAX);
        if exit_code == RG_NO_MATCH_EXIT_CODE {
            return Ok(ToolResult::success(
                ToolResultContent::Text("(no matches)".into()),
                "No matches",
            ));
        }
        if exit_code != 0 {
            let message = ripgrep_failure_message(output.stderr, exit_code);
            return Ok(ToolResult::error(
                ToolResultContent::Text(message.clone()),
                "Find failed",
                ToolCallError::ExecutionFailed(message),
            ));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let Some((matches, count)) = format_glob_matches(&text) else {
            return Ok(ToolResult::success(
                ToolResultContent::Text("(no matches)".into()),
                "No matches",
            ));
        };

        Ok(ToolResult::success(
            ToolResultContent::Text(matches),
            format!("{count} matches"),
        ))
    }
}

fn format_glob_matches(text: &str) -> Option<(String, usize)> {
    if text.is_empty() {
        return None;
    }
    if !text.as_bytes().contains(&b'\r') {
        let newline_count = text.bytes().filter(|byte| *byte == b'\n').count();
        let count = newline_count + usize::from(!text.ends_with('\n'));
        let matches = text.strip_suffix('\n').unwrap_or(text).to_string();
        return Some((matches, count));
    }

    let mut count = 0usize;
    let mut matches = String::with_capacity(text.len());
    for line in text.lines() {
        if count > 0 {
            matches.push('\n');
        }
        matches.push_str(line);
        count += 1;
    }
    (count > 0).then_some((matches, count))
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn format_glob_matches_handles_empty_output() {
        assert_eq!(format_glob_matches(""), None);
    }

    #[test]
    fn format_glob_matches_preserves_paths_and_count() {
        assert_eq!(
            format_glob_matches("src/lib.rs\nsrc/main.rs\n"),
            Some(("src/lib.rs\nsrc/main.rs".to_string(), 2))
        );
    }

    #[test]
    fn format_glob_matches_preserves_crlf_behavior() {
        assert_eq!(
            format_glob_matches("src/lib.rs\r\nsrc/main.rs\r\n"),
            Some(("src/lib.rs\nsrc/main.rs".to_string(), 2))
        );
    }

    #[test]
    #[ignore]
    fn bench_format_glob_matches_many_paths() {
        let text = (0..1_000)
            .map(|idx| format!("crates/core/src/generated/module_{idx}.rs"))
            .collect::<Vec<_>>()
            .join("\n");
        let iterations = 50_000;
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            let (matches, count) =
                black_box(format_glob_matches(black_box(&text))).expect("matches");
            total_len += matches.len() + count;
        }

        let elapsed = started.elapsed();
        assert!(total_len > 0);
        println!(
            "format_glob_matches_many_paths iterations={iterations} bytes={} elapsed_ms={} per_call_us={:.2}",
            text.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }
}
