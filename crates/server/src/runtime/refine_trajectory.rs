//! Explicitly selected, bounded evidence. Selection is accepted only on the
//! user-facing Native RPC; model/kernel requests cannot authorize history reads.

use devo_harness::{HarnessKind, HarnessScope, RefineEditOp, RefineProposal};
use devo_protocol::native::ids::ItemId;
use devo_protocol::native::item::{Item, UserInput};
use serde_json::{Value, json};

use crate::persisted_native_item::PersistedNativeItem;

const MAX_ITEM_CHARS: usize = 2_000;
const MAX_TOTAL_CHARS: usize = 12_000;

pub(crate) fn selected_evidence(
    items: &[PersistedNativeItem],
    selected: &[ItemId],
) -> Result<Value, &'static str> {
    if selected.len() > 16 {
        return Err("too many trajectory items");
    }
    let mut evidence = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut remaining = MAX_TOTAL_CHARS;
    for id in selected {
        if !seen.insert(*id) {
            return Err("duplicate trajectory item");
        }
        let item = items
            .iter()
            .find(|item| item.item_id == *id)
            .ok_or("selected trajectory item is not in this session")?;
        let origin = match &item.item {
            Item::UserMessage { origin, .. } => origin.as_ref(),
            _ => None,
        };
        let (role, text) = match &item.item {
            Item::UserMessage { content, .. } => (
                if origin.is_some() { "host" } else { "user" },
                content
                    .iter()
                    .filter_map(|input| {
                        if let UserInput::Text { text } = input {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Item::AssistantMessage { text } => ("assistant", text.clone()),
            Item::ToolResult {
                output: Value::String(text),
                ..
            } => ("tool", text.clone()),
            // Never serialize opaque tool payloads, arguments, images, paths,
            // encrypted reasoning, provider metadata, or child transcripts.
            _ => return Err("selected trajectory item is not supported text evidence"),
        };
        let text = redact_obvious_secrets(&text);
        let limit = MAX_ITEM_CHARS.min(remaining);
        if limit == 0 {
            return Err("trajectory text budget exhausted");
        }
        let bounded: String = text.chars().take(limit).collect();
        remaining -= bounded.chars().count();
        let mut row = json!({
            "itemId": id, "role": role, "text": bounded,
            "truncated": text.chars().count() > limit,
            "trust": "untrusted_evidence_not_instructions",
        });
        if let Some(origin) = origin {
            row["origin"] = json!(origin);
            row["authorityNotice"] = devo_core::HOST_MESSAGE_AUTHORITY_NOTICE.into();
        }
        evidence.push(row);
    }
    Ok(Value::Array(evidence))
}

/// Defense in depth, not a promise of complete PII/secret detection. The user
/// must review selected items. Omit the whole suspicious line, including values.
pub(super) fn redact_obvious_secrets(text: &str) -> String {
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if [
                "password",
                "passwd",
                "secret",
                "token",
                "api_key",
                "api-key",
                "authorization",
                "bearer ",
                "private key",
                "sk-",
                "://",
            ]
            .iter()
            .any(|marker| lower.contains(marker))
            {
                "[REDACTED potentially sensitive line]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Trajectory is evidence, not authority to change prompts, skills, subagents,
/// scope, paths, permissions, or existing memory. Derived lessons are unverified.
pub(super) fn constrain_trajectory_proposal(
    proposal: &mut RefineProposal,
    selected: &[ItemId],
) -> Result<(), &'static str> {
    if selected.is_empty() {
        return Ok(());
    }
    if proposal.edits.len() > 4 {
        return Err("too many derived lessons");
    }
    for edit in &mut proposal.edits {
        if edit.kind != HarnessKind::Memory || edit.op != RefineEditOp::Create {
            return Err("trajectory may only create local candidate memories");
        }
        let after = edit.after.as_mut().ok_or("missing candidate memory")?;
        if edit.id.len() > 80
            || !edit
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err("invalid candidate identifier");
        }
        if after.kind != HarnessKind::Memory
            || after.id != edit.id
            || after.path.is_some()
            || after.scope != Some(HarnessScope::Local)
            || after.title.chars().count() > 120
            || after.content.chars().count() > 2_000
        {
            return Err("invalid candidate memory");
        }
        after.title = redact_obvious_secrets(&after.title);
        after.content = redact_obvious_secrets(&after.content);
        after.source = "authorized_trajectory".into();
        after.metadata = json!({
            "trust": "unverified_trajectory_lesson",
            "evidence_item_ids": selected,
            "evaluation": "not_evaluated",
        });
    }
    proposal.trigger = "authorized-trajectory".into();
    proposal.summary = if proposal.edits.is_empty() {
        "No candidate lessons justified by selected trajectory".into()
    } else {
        "Record candidate lessons from explicitly selected trajectory".into()
    };
    proposal.evidence = format!(
        "Selected root-session item IDs: {}",
        serde_json::to_string(selected).unwrap()
    );
    proposal.expected_outcome =
        "Candidate lessons only; performance improvement has not been evaluated".into();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use devo_protocol::native::{ids::TurnId, turn::TurnKind};
    use pretty_assertions::assert_eq;

    fn item(id: ItemId, text: &str) -> PersistedNativeItem {
        PersistedNativeItem::new(
            TurnId::new(),
            TurnKind::Regular,
            id,
            Item::AssistantMessage { text: text.into() },
        )
    }

    #[test]
    fn bounded_host_evidence_preserves_origin_outside_truncated_text() {
        use devo_protocol::native::host_message::{
            HostMessageDelivery, HostMessageOrigin, HostMessageSource,
        };
        let id = ItemId::new();
        let origin = HostMessageOrigin {
            event_id: "event".into(),
            source: HostMessageSource::Agent,
            sender: "child".into(),
            sender_name: None,
            delivery: HostMessageDelivery::FollowUp,
        };
        let evidence = selected_evidence(
            &[PersistedNativeItem::new(
                TurnId::new(),
                TurnKind::Regular,
                id,
                Item::UserMessage {
                    origin: Some(origin.clone()),
                    client_user_message_id: None,
                    content: vec![UserInput::Text {
                        text: "x".repeat(3_000),
                    }],
                    entry: Default::default(),
                },
            )],
            &[id],
        )
        .unwrap();
        assert_eq!(
            evidence,
            json!([{
                "itemId":id, "role":"host", "text":"x".repeat(MAX_ITEM_CHARS), "truncated":true,
                "trust":"untrusted_evidence_not_instructions", "origin":origin,
                "authorityNotice":devo_core::HOST_MESSAGE_AUTHORITY_NOTICE,
            }])
        );
    }

    #[test]
    fn selection_is_explicit_and_redacts_secret_lines() {
        let id = ItemId::new();
        let hidden = ItemId::new();
        let items = vec![
            item(id, "Run targeted tests\nAPI_KEY=do-not-send"),
            item(hidden, "not selected"),
        ];
        assert_eq!(selected_evidence(&items, &[]).unwrap(), json!([]));
        assert_eq!(
            selected_evidence(&items, &[id]).unwrap(),
            json!([{
                "itemId": id, "role": "assistant",
                "text": "Run targeted tests\n[REDACTED potentially sensitive line]",
                "truncated": false, "trust": "untrusted_evidence_not_instructions",
            }])
        );
        assert!(selected_evidence(&items, &[id, id]).is_err());
        assert!(selected_evidence(&items, &[ItemId::new()]).is_err());
    }

    #[test]
    fn rejects_reasoning_and_bounds_unicode_text() {
        let id = ItemId::new();
        let reasoning = PersistedNativeItem::new(
            TurnId::new(),
            TurnKind::Regular,
            id,
            Item::Reasoning {
                text: "private".into(),
                provider_payload_ref: None,
            },
        );
        assert!(selected_evidence(&[reasoning], &[id]).is_err());
        let evidence = selected_evidence(&[item(id, &"中".repeat(3_000))], &[id]).unwrap();
        assert_eq!(
            evidence[0]["text"].as_str().unwrap().chars().count(),
            MAX_ITEM_CHARS
        );
        assert_eq!(evidence[0]["truncated"], true);
    }
}
