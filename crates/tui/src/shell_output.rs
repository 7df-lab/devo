//! Display cleanup for shell-family tool output (`bash` / `shell_command` /
//! `exec_command`).
//!
//! Shell tools return their result as `<stdout>\n<envelope-json>`, where the
//! envelope carries display metadata (`{output, command, exit, description,
//! cwd, yield_time_ms}`). Strip the envelope so transcript views show only the
//! real command output; the command and description are rendered separately.

/// Strips the shell result envelope from tool output text.
///
/// Returns the real command output when `text` is either the envelope itself
/// (the command produced no stdout) or `<stdout>\n<envelope-json>`. Otherwise
/// returns `text` unchanged.
pub(crate) fn strip_shell_envelope(text: &str) -> String {
    // The output IS the envelope (the command produced no stdout).
    if let Some(body) = parse_envelope(text.trim()) {
        return body.unwrap_or_default();
    }
    // Stdout followed by the envelope JSON (they are joined with "\n").
    if let Some(separator) = text.rfind("\n{")
        && parse_envelope(text[separator + 1..].trim()).is_some()
    {
        return text[..separator].trim_end().to_string();
    }
    text.to_string()
}

/// Parses a shell envelope JSON object, returning its embedded `output` field.
fn parse_envelope(candidate: &str) -> Option<Option<String>> {
    if !candidate.starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(candidate).ok()?;
    let object = value.as_object()?;
    let has_command = object.get("command").is_some_and(|v| v.is_string())
        || object.get("cmd").is_some_and(|v| v.is_string());
    let has_exit = object.get("exit").is_some_and(|v| v.is_number());
    if !has_command || !has_exit {
        return None;
    }
    Some(
        object
            .get("output")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn strips_envelope_only_output() {
        let envelope = r#"{"output":"","command":"ls","exit":0,"description":"List files","cwd":"/repo","yield_time_ms":1000}"#;
        assert_eq!(strip_shell_envelope(envelope), "");
    }

    #[test]
    fn strips_trailing_envelope_after_stdout() {
        let envelope = r#"{"output":"files","command":"ls","exit":0}"#;
        assert_eq!(
            strip_shell_envelope(&format!("hello\nworld\n{envelope}")),
            "hello\nworld"
        );
    }

    #[test]
    fn keeps_non_envelope_output() {
        assert_eq!(strip_shell_envelope("just text"), "just text");
        assert_eq!(strip_shell_envelope(r#"{"foo": 1}"#), r#"{"foo": 1}"#);
    }

    #[test]
    fn keeps_json_output_with_embedded_newline_brace() {
        // Real JSON/text that happens to contain `\n{` must not be truncated
        // unless the trailing segment is a shell envelope.
        let text = "report:\n{\"files\": 3}\nmore";
        assert_eq!(strip_shell_envelope(text), text);
    }

    #[test]
    fn returns_envelope_output_when_no_stdout() {
        let envelope = r#"{"output":"files","cmd":"ls","exit":0}"#;
        assert_eq!(strip_shell_envelope(envelope), "files");
    }
}
