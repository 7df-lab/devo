//! Shared JSON object slice helper for LLM reply parsing.

/// Returns the first top-level `{...}` object in `raw`, respecting strings.
pub(crate) fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        let index = start + offset;
        let ch = *byte as char;
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::extract_json_object;

    #[test]
    fn extracts_nested_object_without_trailing_text() {
        assert_eq!(
            extract_json_object("prefix {\"a\":{\"b\":1}} trailing"),
            Some("{\"a\":{\"b\":1}}")
        );
    }

    #[test]
    fn ignores_braces_inside_strings() {
        assert_eq!(
            extract_json_object(r#"{"text":"not } a closer"}"#),
            Some(r#"{"text":"not } a closer"}"#)
        );
    }
}
