use std::path::PathBuf;

use super::UserMessage;

#[derive(Clone, Debug, Default)]
pub(super) struct RealtimeConversationUiState;

#[derive(Clone, Debug, Default)]
pub(super) struct PendingSteerCompareKey;

#[derive(Clone, Debug)]
pub(super) struct RenderedUserMessageEvent {
    pub(super) text: String,
    pub(super) local_images: Vec<PathBuf>,
}

impl RenderedUserMessageEvent {
    pub(super) fn from_user_message(user_message: &UserMessage) -> Self {
        Self {
            text: user_message.text.clone(),
            local_images: user_message
                .local_images
                .iter()
                .map(|image| image.path.clone())
                .collect(),
        }
    }
}
