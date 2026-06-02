//! Git diff display rules for the chat widget.
//!
//! This module keeps the small pure rules for `/diff` rendering and automatic
//! diff display separate from the stateful conversation widget.

pub(super) fn format_git_diff_result(result: std::io::Result<(bool, String)>) -> String {
    match result {
        Ok((true, diff_text)) => {
            if diff_text.trim().is_empty() {
                "No changes detected.".to_string()
            } else {
                diff_text
            }
        }
        Ok((false, _)) => "`/diff` — _not inside a git repository_".to_string(),
        Err(err) => format!("Failed to compute diff: {err}"),
    }
}

pub(super) fn should_auto_show_git_diff(tool_title: &str, is_error: bool) -> bool {
    if is_error {
        return false;
    }
    let lower = tool_title.to_ascii_lowercase();
    lower.contains("write ")
        || lower.starts_with("write:")
        || lower.contains("edit ")
        || lower.starts_with("edit:")
        || lower.contains("apply_patch")
        || lower.contains("apply patch")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn format_git_diff_result_handles_empty_and_non_repo_cases() {
        assert_eq!(
            format_git_diff_result(Ok((true, String::new()))),
            "No changes detected."
        );
        assert_eq!(
            format_git_diff_result(Ok((false, String::new()))),
            "`/diff` — _not inside a git repository_"
        );
    }

    #[test]
    fn auto_diff_only_matches_successful_editing_tools() {
        let cases = [
            ("Write file", false, true),
            ("edit: config", false, true),
            ("apply_patch", false, true),
            ("Read file", false, false),
            ("Write file", true, false),
        ];
        let actual: Vec<_> = cases
            .into_iter()
            .map(|(title, is_error, _expected)| should_auto_show_git_diff(title, is_error))
            .collect();

        assert_eq!(actual, vec![true, true, true, false, false]);
    }
}
