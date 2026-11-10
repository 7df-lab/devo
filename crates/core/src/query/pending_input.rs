//! Insert pending steering inputs into the next model request without losing media.

use devo_protocol::PendingInputItem;

use crate::ContentBlock;
use crate::Message;
use crate::Role;
use crate::SessionState;

pub(super) fn push_pending_input(session: &mut SessionState, pending: &[PendingInputItem]) {
    for item in pending {
        match &item.kind {
            devo_protocol::PendingInputKind::UserText { text } => {
                session.push_message(Message::user(text.clone()));
            }
            devo_protocol::PendingInputKind::UserInput {
                prompt_text,
                prompt_messages,
                prompt_images,
                ..
            } => {
                if prompt_images.is_empty() {
                    if prompt_messages.is_empty() {
                        session.push_message(Message::user(prompt_text.clone()));
                    } else {
                        for prompt_message in prompt_messages {
                            session.push_message(Message::user(prompt_message.clone()));
                        }
                    }
                } else {
                    // Match initial-turn input: ordered text parts, then images,
                    // in one user message. Image-only input needs no empty text.
                    let text_parts = if prompt_messages.is_empty() {
                        std::slice::from_ref(prompt_text)
                    } else {
                        prompt_messages.as_slice()
                    };
                    let mut content: Vec<ContentBlock> = text_parts
                        .iter()
                        .filter(|text| !text.trim().is_empty())
                        .map(|text| ContentBlock::Text { text: text.clone() })
                        .collect();
                    content.extend(prompt_images.iter().map(|image| ContentBlock::Image {
                        mime_type: image.mime_type.clone(),
                        data_base64: image.data_base64.clone(),
                    }));
                    session.push_message(Message {
                        role: Role::User,
                        content,
                    });
                }
            }
            devo_protocol::PendingInputKind::ToolCallBlockedByHook {
                tool_use_id,
                reason,
            } => {
                session.push_message(Message::user(format!(
                    "[Tool call {tool_use_id} was blocked: {reason}]"
                )));
            }
            devo_protocol::PendingInputKind::BudgetLimitSteering => {
                session.push_message(Message::system(
                    "Note: The conversation is approaching the token budget limit. \
                     Please be concise and consider wrapping up the current task.",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use devo_protocol::PendingInputKind;
    use devo_protocol::PromptImagePart;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;
    use crate::SessionConfig;

    fn user_input(
        prompt_text: &str,
        prompt_messages: &[&str],
        prompt_images: Vec<PromptImagePart>,
    ) -> PendingInputItem {
        PendingInputItem::new(
            PendingInputKind::UserInput {
                input: Vec::new(),
                display_text: "display text is not the model prompt".into(),
                prompt_text: prompt_text.into(),
                prompt_messages: prompt_messages.iter().map(|text| (*text).into()).collect(),
                prompt_images,
            },
            /*metadata*/ None,
            chrono::Utc::now(),
        )
    }

    #[test]
    fn pending_images_preserve_multi_message_and_image_order_in_model_input() {
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        // Exercise the separate model prompt projection used by live turns too.
        session.set_prompt_messages(vec![Message::user("existing")]);
        push_pending_input(
            &mut session,
            &[
                user_input(
                    "fallback must not replace message parts",
                    &["first", " ", "second"],
                    vec![
                        PromptImagePart {
                            mime_type: "image/png".into(),
                            data_base64: "cG5n".into(),
                        },
                        PromptImagePart {
                            mime_type: "image/jpeg".into(),
                            data_base64: "anBlZw==".into(),
                        },
                    ],
                ),
                user_input("next item", &[], Vec::new()),
            ],
        );
        assert_eq!(
            serde_json::to_value(session.to_request_messages()).unwrap(),
            json!([
                {"role": "user", "content": [{"type": "text", "text": "existing"}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "first"},
                    {"type": "text", "text": "second"},
                    {"type": "image", "mime_type": "image/png", "data_base64": "cG5n"},
                    {"type": "image", "mime_type": "image/jpeg", "data_base64": "anBlZw=="}
                ]},
                {"role": "user", "content": [{"type": "text", "text": "next item"}]}
            ])
        );
    }

    #[test]
    fn pending_image_only_input_reaches_model_without_empty_text() {
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        push_pending_input(
            &mut session,
            &[user_input(
                " \n",
                &[],
                vec![PromptImagePart {
                    mime_type: "image/png".into(),
                    data_base64: "cG5n".into(),
                }],
            )],
        );
        assert_eq!(
            serde_json::to_value(session.to_request_messages()).unwrap(),
            json!([{"role": "user", "content": [
                {"type": "image", "mime_type": "image/png", "data_base64": "cG5n"}
            ]}])
        );
    }

    #[test]
    fn pending_text_only_input_preserves_separate_messages_and_fallback() {
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        push_pending_input(
            &mut session,
            &[
                user_input("unused fallback", &["first", "second"], Vec::new()),
                user_input("fallback", &[], Vec::new()),
            ],
        );
        assert_eq!(
            session.prompt_source_messages(),
            &[
                Message::user("first"),
                Message::user("second"),
                Message::user("fallback"),
            ]
        );
    }

    #[test]
    fn pending_images_include_fallback_prompt_text() {
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        push_pending_input(
            &mut session,
            &[user_input(
                "describe this",
                &[],
                vec![PromptImagePart {
                    mime_type: "image/png".into(),
                    data_base64: "cG5n".into(),
                }],
            )],
        );
        assert_eq!(
            serde_json::to_value(session.to_request_messages()).unwrap(),
            json!([{"role": "user", "content": [
                {"type": "text", "text": "describe this"},
                {"type": "image", "mime_type": "image/png", "data_base64": "cG5n"}
            ]}])
        );
    }
}
