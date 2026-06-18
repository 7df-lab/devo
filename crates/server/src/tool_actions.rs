use std::path::Path;
use std::path::PathBuf;

use devo_protocol::parse_command::ParsedCommand;

pub(crate) fn read_action_from_tool_input(
    command: &str,
    input: &serde_json::Value,
) -> Option<ParsedCommand> {
    let path = input
        .get("filePath")
        .or_else(|| input.get("path"))
        .and_then(serde_json::Value::as_str)?
        .trim();
    if path.is_empty() {
        return None;
    }

    read_action_from_path(command.to_string(), path)
}

fn read_action_from_path(cmd: String, path: &str) -> Option<ParsedCommand> {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string());

    Some(ParsedCommand::Read {
        cmd,
        name,
        path: PathBuf::from(path),
    })
}

pub(crate) fn read_action_from_tool_summary(summary: &str) -> Option<ParsedCommand> {
    let path = summary
        .strip_prefix("read: ")
        .or_else(|| summary.strip_prefix("read "))
        .unwrap_or_default()
        .trim();
    let path = path
        .split_once(" (offset:")
        .or_else(|| path.split_once(" (limit:"))
        .map_or(path, |(path, _)| path)
        .trim();
    if path.is_empty() {
        return None;
    }

    read_action_from_path(summary.replacen(": ", " ", 1), path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn read_action_from_tool_input_uses_file_path() {
        let input = serde_json::json!({ "filePath": "src/main.rs" });

        assert_eq!(
            read_action_from_tool_input("read", &input),
            Some(ParsedCommand::Read {
                cmd: "read".to_string(),
                name: "main.rs".to_string(),
                path: PathBuf::from("src/main.rs"),
            })
        );
    }

    #[test]
    fn read_action_from_tool_summary_strips_display_suffix() {
        assert_eq!(
            read_action_from_tool_summary("read: src/main.rs (offset: 20)"),
            Some(ParsedCommand::Read {
                cmd: "read src/main.rs (offset: 20)".to_string(),
                name: "main.rs".to_string(),
                path: PathBuf::from("src/main.rs"),
            })
        );
    }
}
