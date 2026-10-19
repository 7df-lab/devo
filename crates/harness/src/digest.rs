//! Prompt digest for harness state (strip/reattach format).
//!
//! Digests are regenerated on the compaction head: strip them from summarizer
//! input and reattach a fresh digest afterward (`strip_harness_digests` /
//! `reattach_harness_digest`).

use crate::state::{HarnessKind, HarnessState};

/// Digest marker prefix (strip/reattach key).
pub const HARNESS_DIGEST_PREFIX: &str = "[harness-digest]\n\nThe persistent memories produced across this session so far:\n\n<harness_state>\n";

/// Digest marker suffix.
pub const HARNESS_DIGEST_SUFFIX: &str = "\n</harness_state>";

/// Heading used by [`format_harness_state_for_prompt`] (also treated as digest).
pub const HARNESS_DIGEST_HEADING: &str = "## Continual harness";

/// Format \(H\) for injection into the next turn context (after skills, before goal).
pub fn format_harness_state_for_prompt(state: &HarnessState) -> String {
    let body = format_harness_body(state);
    if body.is_empty() {
        return String::new();
    }
    wrap_harness_digest(&format!(
        "{HARNESS_DIGEST_HEADING}\n\n{body}\n\nWhen evidence warrants a durable update, call `await refine.run()`.\n"
    ))
}

fn format_harness_body(state: &HarnessState) -> String {
    let mut sections = Vec::new();
    for (kind, label) in [
        (HarnessKind::Prompt, "Prompt notes"),
        (HarnessKind::Memory, "Memories"),
        (HarnessKind::Skill, "Harness skills"),
        (HarnessKind::Subagent, "Subagent specs"),
    ] {
        let key = match kind {
            HarnessKind::Prompt => "prompt",
            HarnessKind::Memory => "memory",
            HarnessKind::Skill => "skill",
            HarnessKind::Subagent => "subagent",
        };
        let Some(bucket) = state.entries.get(key) else {
            continue;
        };
        if bucket.is_empty() {
            continue;
        }
        let mut lines = vec![format!("### {label}")];
        for entry in bucket.values() {
            let scope = if entry.scope == Some(crate::state::HarnessScope::Global) {
                " (global)"
            } else {
                ""
            };
            lines.push(format!("- [{}] {}{scope}", entry.id, entry.title));
            if !entry.content.is_empty() {
                lines.push(format!("  {}", entry.content.replace('\n', "\n  ")));
            }
        }
        sections.push(lines.join("\n"));
    }
    sections.join("\n\n")
}

/// Wrap a digest body in markers for strip/reattach.
pub fn wrap_harness_digest(digest_body: &str) -> String {
    if digest_body.is_empty() {
        return String::new();
    }
    format!("{HARNESS_DIGEST_PREFIX}{digest_body}{HARNESS_DIGEST_SUFFIX}")
}

/// True when `text` is (or contains) a harness digest block.
pub fn text_contains_harness_digest(text: &str) -> bool {
    text.contains("[harness-digest]")
        || text.contains("<harness_state>")
        || text.contains(HARNESS_DIGEST_HEADING)
}

/// Remove harness digest blocks from summarizer input text.
///
/// Prefer filtering whole messages via [`is_harness_digest_only`]; this helper
/// covers digests embedded inside larger developer/system blobs.
pub fn strip_harness_digests(text: &str) -> String {
    let mut remaining = text.to_string();
    while let Some(start) = remaining.find("[harness-digest]") {
        let after_prefix = &remaining[start..];
        let end = after_prefix
            .find("</harness_state>")
            .map(|rel| start + rel + "</harness_state>".len())
            .unwrap_or(remaining.len());
        remaining.replace_range(start..end, "");
    }
    // Fallback: unwrapped Devo heading blocks (pre-marker digests).
    if let Some(start) = remaining.find(HARNESS_DIGEST_HEADING) {
        let rest = &remaining[start..];
        let end = rest
            .find("\n## ")
            .or_else(|| rest.find("\n# "))
            .map(|rel| start + rel)
            .unwrap_or(remaining.len());
        remaining.replace_range(start..end, "");
    }
    remaining.trim().to_string()
}

/// True when the entire message is a harness digest (safe to drop from summarizer).
pub fn is_harness_digest_only(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    strip_harness_digests(trimmed).is_empty()
}

/// Filter message texts, dropping digest-only entries and stripping embedded digests.
pub fn filter_texts_for_summarizer<I, S>(texts: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    texts
        .into_iter()
        .filter_map(|text| {
            let text = text.as_ref();
            if is_harness_digest_only(text) {
                return None;
            }
            let stripped = strip_harness_digests(text);
            if stripped.is_empty() {
                None
            } else {
                Some(stripped)
            }
        })
        .collect()
}

/// Reattach a fresh digest ahead of compacted history texts (new head).
///
/// Returns `digest` alone when `history_texts` is empty; otherwise
/// `[digest, …history]`. Empty digests are omitted.
pub fn reattach_harness_digest(digest: &str, history_texts: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(history_texts.len().saturating_add(1));
    if !digest.trim().is_empty() {
        out.push(digest.to_string());
    }
    out.extend(history_texts.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{HarnessEntry, HarnessScope};
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: empty harness produces an empty digest (no empty heading spam).
    #[test]
    fn empty_state_digest_is_empty() {
        assert_eq!(
            format_harness_state_for_prompt(&HarnessState::default()),
            ""
        );
    }

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: digest includes memory entries and refine.run guidance.
    #[test]
    fn digest_includes_memory_and_refine_hint() {
        let mut state = HarnessState::default();
        let now = Utc::now();
        state.entries.get_mut("memory").unwrap().insert(
            "m1".into(),
            HarnessEntry {
                id: "m1".into(),
                kind: HarnessKind::Memory,
                title: "pref".into(),
                content: "use tabs".into(),
                path: None,
                scope: Some(HarnessScope::Local),
                reference: serde_json::json!({}),
                arguments: serde_json::json!({}),
                metadata: serde_json::json!({}),
                source: "test".into(),
                created_at: now,
                updated_at: now,
                version: 1,
            },
        );
        let digest = format_harness_state_for_prompt(&state);
        assert!(digest.contains("use tabs"));
        assert!(digest.contains("refine.run"));
        assert!(digest.contains("[harness-digest]"));
    }

    /// Trace: L2-DES-CONTEXT-002, L2-DES-HARNESS-001
    /// Verifies: digest blocks are excluded from summarizer input and reattached.
    #[test]
    fn strip_and_reattach_digest() {
        let digest = wrap_harness_digest("## Continual harness\n\n- keep me out of summarizer");
        let history = [
            digest.clone(),
            "user: do the work".into(),
            "assistant: ok".into(),
        ];
        let for_summarizer = filter_texts_for_summarizer(history.iter().map(String::as_str));
        assert_eq!(
            for_summarizer,
            vec!["user: do the work".to_string(), "assistant: ok".to_string()]
        );
        let fresh = wrap_harness_digest("## Continual harness\n\n- refreshed");
        let reattached = reattach_harness_digest(&fresh, &for_summarizer);
        assert_eq!(reattached[0], fresh);
        assert_eq!(reattached.len(), 3);
        assert!(strip_harness_digests(&digest).is_empty());
    }
}
