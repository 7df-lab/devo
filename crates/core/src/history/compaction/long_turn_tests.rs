use super::*;
use crate::{ContentBlock, Message};
use pretty_assertions::assert_eq;

struct Checkpoint;

#[async_trait]
impl HistorySummarizer for Checkpoint {
    async fn summarize(
        &self,
        _: Vec<RequestMessage>,
        _: Option<&CancellationToken>,
    ) -> Result<String, CompactionError> {
        Ok("Continue investigating.".into())
    }
}

#[tokio::test]
async fn ten_thousand_tool_steps_keep_one_checkpoint_and_both_human_instructions() {
    let root = ResponseItem::Message(Message::user("Track this task through completion."));
    let amendment = ResponseItem::Message(Message::user("Preserve the original logs."));
    let mut items = vec![root.clone()];
    let mut pins = vec![root.clone()];
    let config = CompactionConfig {
        kind: CompactionKind::OverflowRecovery,
        ..Default::default()
    };
    for step in 0..10_000 {
        if step == 1 {
            pins.push(amendment.clone());
            items.push(amendment.clone());
        }
        let id = format!("tool-{step}");
        items.extend([
            ResponseItem::Reason {
                text: "consider the next observation".into(),
            },
            ResponseItem::Message(Message::assistant_text("Checking the evidence.")),
            ResponseItem::ToolCall {
                id: id.clone(),
                name: "read".into(),
                input: serde_json::json!({"step":step}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: id.clone(),
                content: "observed".into(),
                is_error: false,
            },
            ResponseItem::Message(Message::user("Please continue from where you left off.")),
        ]);
        if step % 4 == 3 {
            let action = compact_history_with_inputs(
                &items,
                &TokenInfo::default(),
                &Checkpoint,
                &config,
                &pins,
                /*prefix*/ None,
                /*cancel_token*/ None,
            )
            .await
            .unwrap();
            let CompactAction::Replaced(next) = action else {
                panic!("checkpoint expected")
            };
            assert_eq!(&next[1..], pins.as_slice());
            let ResponseItem::Message(msg) = &next[0] else {
                panic!("summary expected")
            };
            let [ContentBlock::Text { text }] = msg.content.as_slice() else {
                panic!("summary text expected")
            };
            assert_eq!(text.matches("<compaction_summary>").count(), 1);
            assert!(text.contains(&format!("call:{id}")));
            assert!(text.len() < 1500);
            items = next;
        }
    }
}
