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

    let offset = input.get("offset").and_then(serde_json::Value::as_u64);
    let limit = input.get("limit").and_then(serde_json::Value::as_u64);
    read_action_from_path(command.to_string(), path, offset, limit)
}

fn read_action_from_path(
    cmd: String,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Option<ParsedCommand> {
    let name = format_read_name(path, offset, limit);

    Some(ParsedCommand::Read {
        cmd,
        name,
        path: PathBuf::from(path),
    })
}

pub(crate) fn read_action_from_tool_summary(summary: &str) -> Option<ParsedCommand> {
    let raw_path = summary
        .strip_prefix("read: ")
        .or_else(|| summary.strip_prefix("read "))
        .unwrap_or_default()
        .trim();
    let (path, range) = raw_path
        .split_once(" (")
        .map_or((raw_path, None), |(path, suffix)| (path, Some(suffix)));
    let path = path.trim();
    if path.is_empty() {
        return None;
    }

    let (offset, limit) = range.map_or((None, None), parse_read_range);
    read_action_from_path(summary.replacen(": ", " ", 1), path, offset, limit)
}

fn format_read_name(path: &str, offset: Option<u64>, limit: Option<u64>) -> String {
    let mut name = path.to_string();
    match (offset, limit) {
        (Some(offset), Some(limit)) => {
            let end = offset.saturating_add(limit.saturating_sub(1));
            name.push_str(&format!(" L:{offset}-{end}"));
        }
        (Some(offset), None) => name.push_str(&format!(" L:{offset}-")),
        (None, Some(limit)) => name.push_str(&format!(" L:1-{limit}")),
        (None, None) => {}
    }
    name
}

fn parse_read_range(suffix: &str) -> (Option<u64>, Option<u64>) {
    let suffix = suffix.trim_end_matches(')').trim();
    let mut offset = None;
    let mut limit = None;
    for part in suffix.split(", ") {
        if let Some(value) = part.strip_prefix("offset:") {
            offset = value.trim().parse().ok();
        } else if let Some(value) = part.strip_prefix("limit:") {
            limit = value.trim().parse().ok();
        }
    }
    (offset, limit)
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
                name: "src/main.rs".to_string(),
                path: PathBuf::from("src/main.rs"),
            })
        );
    }

    #[test]
    fn read_action_from_tool_input_formats_inclusive_line_range() {
        let input = serde_json::json!({
            "filePath": "src/main.rs",
            "offset": 20,
            "limit": 10,
        });

        assert_eq!(
            read_action_from_tool_input("read", &input),
            Some(ParsedCommand::Read {
                cmd: "read".to_string(),
                name: "src/main.rs L:20-29".to_string(),
                path: PathBuf::from("src/main.rs"),
            })
        );
    }

    #[test]
    fn read_action_from_tool_input_formats_partial_line_range() {
        let input = serde_json::json!({
            "filePath": "src/main.rs",
            "offset": 20,
        });

        assert_eq!(
            read_action_from_tool_input("read", &input),
            Some(ParsedCommand::Read {
                cmd: "read".to_string(),
                name: "src/main.rs L:20-".to_string(),
                path: PathBuf::from("src/main.rs"),
            })
        );
    }

    #[test]
    fn read_action_from_tool_summary_keeps_display_suffix() {
        assert_eq!(
            read_action_from_tool_summary("read: src/main.rs (offset: 20)"),
            Some(ParsedCommand::Read {
                cmd: "read src/main.rs (offset: 20)".to_string(),
                name: "src/main.rs L:20-".to_string(),
                path: PathBuf::from("src/main.rs"),
            })
        );
    }

    #[test]
    fn read_action_from_tool_summary_formats_limit_only_range() {
        assert_eq!(
            read_action_from_tool_summary("read: src/main.rs (limit: 5)"),
            Some(ParsedCommand::Read {
                cmd: "read src/main.rs (limit: 5)".to_string(),
                name: "src/main.rs L:1-5".to_string(),
                path: PathBuf::from("src/main.rs"),
            })
        );
    }
}
