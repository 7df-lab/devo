//! Small helpers for interpreting tool results inside the research runtime.
//!
//! Research turns treat write-tool output specially so a final report written to
//! disk can still be surfaced as the final answer. The canonical path comes from
//! the write handler's output metadata, but older or fallback flows may only have
//! the original tool input available.

use serde_json::Value;

pub(crate) fn is_write_tool_name(tool_name: &str) -> bool {
    matches!(tool_name, "write" | "write_tool")
}

pub(crate) fn extract_written_file_path(input: &Value, output: &Value) -> Option<String> {
    output
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| files.iter().find_map(path_from_object))
        .or_else(|| path_from_object(output))
        .or_else(|| path_from_object(input))
}

fn path_from_object(value: &Value) -> Option<String> {
    value
        .get("path")
        .or_else(|| value.get("filePath"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn write_tool_name_accepts_current_and_legacy_names() {
        assert!(is_write_tool_name("write"));
        assert!(is_write_tool_name("write_tool"));
        assert!(!is_write_tool_name("read"));
    }

    #[test]
    fn extract_written_file_path_prefers_output_metadata() {
        let input = serde_json::json!({ "filePath": "requested.md" });
        let output = serde_json::json!({
            "files": [
                {
                    "path": "actual.md",
                    "kind": "add"
                }
            ]
        });

        assert_eq!(
            extract_written_file_path(&input, &output),
            Some("actual.md".to_string())
        );
    }

    #[test]
    fn extract_written_file_path_falls_back_to_input() {
        let input = serde_json::json!({ "filePath": "fallback.md" });
        let output = serde_json::json!({ "files": [] });

        assert_eq!(
            extract_written_file_path(&input, &output),
            Some("fallback.md".to_string())
        );
    }
}
