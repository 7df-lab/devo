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
use crate::tool_spec::{ToolCapabilityTag, ToolExecutionMode, ToolOutputMode, ToolSpec};

const MAX_RESULTS: usize = 500;
const TRUNCATED_MESSAGE: &str = "(truncated at 500 matches)";
const GREP_DESCRIPTION: &str = include_str!("../grep.txt");
const REGEX_PARSE_ERROR: &[u8] = b"regex parse error";

pub struct GrepHandler {
    spec: ToolSpec,
}

impl Default for GrepHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl GrepHandler {
    pub fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "grep".into(),
                description: GREP_DESCRIPTION.into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "pattern".to_string(),
                            JsonSchema::string(Some(
                                "The regex pattern to search for in file contents",
                            )),
                        ),
                        (
                            "path".to_string(),
                            JsonSchema::string(Some("The directory to search in")),
                        ),
                        (
                            "include".to_string(),
                            JsonSchema::string(Some("File pattern to include in the search")),
                        ),
                        (
                            "case_insensitive".to_string(),
                            JsonSchema::boolean(Some("Search without case sensitivity")),
                        ),
                    ]),
                    Some(vec!["pattern".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![ToolCapabilityTag::SearchWorkspace],
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
impl ToolHandler for GrepHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let pattern_str = input["pattern"]
            .as_str()
            .ok_or_else(|| ToolCallError::InvalidInput("missing 'pattern' field".into()))?;

        let case_insensitive = input["case_insensitive"].as_bool().unwrap_or(false);
        let path = input["path"].as_str().unwrap_or(".");
        let include = input["include"]
            .as_str()
            .or_else(|| input["glob"].as_str())
            .filter(|value| !value.is_empty());
        debug!(pattern = pattern_str, path, include, "grep search");

        let mut args = vec![
            OsString::from("--line-number"),
            OsString::from("--with-filename"),
            OsString::from("--no-heading"),
            OsString::from("--color"),
            OsString::from("never"),
        ];
        if case_insensitive {
            args.push(OsString::from("--ignore-case"));
        }
        if let Some(include) = include {
            args.push(OsString::from("--glob"));
            args.push(OsString::from(include));
        }
        args.push(OsString::from("--"));
        args.push(OsString::from(pattern_str));
        args.push(OsString::from(path));

        let output = run_rg(&ctx, args).await?;
        let exit_code = output.status.code().unwrap_or(i32::MAX);
        if exit_code == RG_NO_MATCH_EXIT_CODE {
            return Ok(ToolResult::success(
                ToolResultContent::Text("(no matches)".into()),
                "No matches",
            ));
        }
        if exit_code != 0 {
            let message = ripgrep_failure_message(output.stderr, exit_code);
            let error = if contains_ascii_case_insensitive(&message, REGEX_PARSE_ERROR) {
                ToolCallError::InvalidInput(message.clone())
            } else {
                ToolCallError::ExecutionFailed(message.clone())
            };
            return Ok(ToolResult::error(
                ToolResultContent::Text(message),
                "Grep failed",
                error,
            ));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let Some((matches, summary)) = format_grep_matches(&text) else {
            return Ok(ToolResult::success(
                ToolResultContent::Text("(no matches)".into()),
                "No matches",
            ));
        };
        Ok(ToolResult::success(
            ToolResultContent::Text(matches),
            summary,
        ))
    }
}

fn format_grep_matches(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    let first = lines.next()?;
    let mut matches = String::with_capacity(text.len().min(64 * 1024));
    matches.push_str(first);
    let mut displayed_count = 1;
    for line in lines.by_ref().take(MAX_RESULTS - 1) {
        matches.push('\n');
        matches.push_str(line);
        displayed_count += 1;
    }
    let truncated = lines.next().is_some();
    if truncated {
        matches.push('\n');
        matches.push_str(TRUNCATED_MESSAGE);
    }
    let summary = if truncated {
        "500+ matches".to_string()
    } else {
        format!("{displayed_count} matches")
    };
    Some((matches, summary))
}

fn contains_ascii_case_insensitive(text: &str, needle: &[u8]) -> bool {
    text.as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn format_grep_matches_handles_empty_output() {
        assert_eq!(format_grep_matches(""), None);
    }

    #[test]
    fn format_grep_matches_preserves_lines_and_summary() {
        assert_eq!(
            format_grep_matches("a:1:one\nb:2:two\n"),
            Some(("a:1:one\nb:2:two".to_string(), "2 matches".to_string()))
        );
    }

    #[test]
    fn format_grep_matches_truncates_after_max_results() {
        let text = (0..=MAX_RESULTS)
            .map(|idx| format!("src/lib.rs:{idx}:match"))
            .collect::<Vec<_>>()
            .join("\n");
        let Some((matches, summary)) = format_grep_matches(&text) else {
            panic!("matches should be present");
        };
        let lines = matches.lines().collect::<Vec<_>>();

        assert_eq!(summary, "500+ matches");
        assert_eq!(lines.len(), MAX_RESULTS + 1);
        assert_eq!(lines[MAX_RESULTS], TRUNCATED_MESSAGE);
    }

    #[test]
    fn regex_parse_error_detection_is_ascii_case_insensitive() {
        assert!(contains_ascii_case_insensitive(
            "REGEX PARSE ERROR near column 1",
            REGEX_PARSE_ERROR
        ));
        assert!(!contains_ascii_case_insensitive(
            "permission denied",
            REGEX_PARSE_ERROR
        ));
    }

    #[test]
    #[ignore]
    fn bench_format_grep_matches_many_results() {
        let text = (0..600)
            .map(|idx| format!("crates/core/src/lib.rs:{idx}:needle result {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let iterations = 20_000;
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            let (matches, summary) =
                black_box(format_grep_matches(black_box(&text))).expect("matches");
            total_len += matches.len() + summary.len();
        }

        let elapsed = started.elapsed();
        assert!(total_len > 0);
        println!(
            "format_grep_matches_many_results iterations={iterations} bytes={} elapsed_ms={} per_call_us={:.2}",
            text.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }
}
