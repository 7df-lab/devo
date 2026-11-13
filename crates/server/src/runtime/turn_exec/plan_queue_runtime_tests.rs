//! Real Native RPC → core/provider stream → server lifecycle coverage for Plan.
use super::*;
use async_trait::async_trait;
use devo_protocol::{
    ModelRequest, ModelResponse, ResponseContent, ResponseMetadata, StopReason, StreamEvent, Usage,
};
use futures::StreamExt;
use pretty_assertions::assert_eq;
use std::{
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::Semaphore;

struct PlanProvider {
    release: Arc<Semaphore>,
    requests: AtomicUsize,
}
#[async_trait]
impl devo_provider::ModelProviderSDK for PlanProvider {
    fn name(&self) -> &str {
        "plan-boundary-provider"
    }
    async fn completion(&self, _: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("auxiliary completion disabled")
    }
    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        let initial = self.requests.fetch_add(1, Ordering::SeqCst) == 0;
        let text = if initial {
            "Before plan\n<proposed_plan>\nPlan body\n</proposed_plan>\nAfter plan"
        } else {
            "Queued plan reply"
        };
        let response = ModelResponse {
            id: "plan-reply".into(),
            content: vec![ResponseContent::Text(text.into())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        };
        let events = if initial {
            vec![
                StreamEvent::ReasoningStart { index: 0 },
                StreamEvent::ReasoningDelta {
                    index: 0,
                    text: "Plan reasoning".into(),
                },
                StreamEvent::ReasoningDone { index: 0 },
                StreamEvent::TextDelta {
                    index: 1,
                    text: "Before plan\n".into(),
                },
                StreamEvent::TextDelta {
                    index: 1,
                    text: "<proposed_plan>\nPlan body\n</proposed_plan>\nAfter plan".into(),
                },
                StreamEvent::MessageDone { response },
            ]
        } else {
            vec![
                StreamEvent::TextDelta {
                    index: 0,
                    text: text.into(),
                },
                StreamEvent::MessageDone { response },
            ]
        };
        let release = self.release.clone();
        Ok(Box::pin(futures::stream::iter(events).then(move |event| {
            let release = release.clone();
            async move {
                if matches!(&event, StreamEvent::TextDelta { text, .. } if text.starts_with("<proposed_plan>")) {
                    release.acquire().await.context("release gate")?.forget();
                }
                Ok(event)
            }
        })))
    }
}

async fn next_event(receiver: &mut mpsc::Receiver<Value>) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(15), receiver.recv())
        .await
        .context("Native notification timeout")?
        .context("Native notification channel closed")
}

#[tokio::test]
async fn plan_stream_queue_admission_keeps_prose_items_unique_and_complete() -> Result<()> {
    for mode in ["follow", "steer"] {
        let home = tempfile::tempdir()?;
        let provider = Arc::new(PlanProvider {
            release: Arc::new(Semaphore::new(0)),
            requests: AtomicUsize::new(0),
        });
        let (runtime, session, connection, mut receiver) =
            fixture_with_provider(home.path(), provider.clone()).await?;
        let snapshot = runtime
            .handle_incoming(
                connection,
                json!({"id": 30, "method": "session/read", "params": {"sessionId": session}}),
            )
            .await
            .context("read session version")?;
        let version = snapshot["result"]["session"]["version"]
            .as_u64()
            .context("session version")?;
        let configured = runtime
            .handle_incoming(
                connection,
                json!({"id": 3, "method": "session/metadata/update", "params": {
                    "sessionId": session, "expectedVersion": version, "settings": {"mode": "plan"}
                }}),
            )
            .await
            .context("configure Plan mode")?;
        assert!(configured.get("error").is_none(), "{configured}");
        let started = runtime.handle_incoming(connection, json!({"id": 4, "method": "turn/start", "params": {
            "sessionId": session, "input": [{"type":"text","text":"Initial plan request"}], "idempotencyKey":"initial-plan"
        }})).await.context("start Plan turn")?;
        let initial_turn = started["result"]["turn"]["id"]
            .as_str()
            .context("initial turn")?
            .to_string();
        let mut events = Vec::new();
        loop {
            let event = next_event(&mut receiver).await?;
            let reached = event["params"]["delta"] == "Before plan\n";
            events.push(event);
            if reached {
                break;
            }
        }
        let mut params = json!({"sessionId":session,"input":[{"type":"text","text":"Queued plan request"}],"idempotencyKey":"queued-plan"});
        if mode == "steer" {
            params["expectedTurnId"] = json!(initial_turn);
        }
        let admission = runtime.handle_incoming(connection, json!({"id":5,"method":if mode == "steer" {"turn/steer"} else {"session/queue/push"},"params":params})).await.context("queue plan input")?;
        assert!(admission.get("error").is_none(), "{admission}");
        provider.release.add_permits(1);
        let mut queued_turn = None;
        loop {
            let event = next_event(&mut receiver).await?;
            let item = &event["params"]["item"];
            if event["method"] == "item/completed" && item["item"]["text"] == "Queued plan reply" {
                queued_turn = Some(
                    item["turnId"]
                        .as_str()
                        .context("queued reply turn")?
                        .to_string(),
                );
            }
            let finished = queued_turn.as_ref().is_some_and(|turn| {
                event["method"] == "turn/completed"
                    && event["params"]["turn"]["id"] == *turn
                    && event["params"]["turn"]["status"] == "completed"
            });
            events.push(event);
            if finished {
                break;
            }
        }
        let queued_turn = queued_turn.context("queued reply")?;
        assert_eq!(queued_turn == initial_turn, mode == "steer");
        assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
        let lifecycle: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    event["method"].as_str(),
                    Some("item/started" | "item/completed")
                )
            })
            .collect();
        let mut prose_ids = Vec::new();
        for text in ["Before plan\n", "After plan", "Queued plan reply"] {
            let completed: Vec<_> = lifecycle
                .iter()
                .filter(|event| {
                    event["method"] == "item/completed"
                        && event["params"]["item"]["item"]["text"] == text
                })
                .collect();
            assert_eq!(completed.len(), 1, "completion count for {text}");
            let envelope = &completed[0]["params"]["item"];
            assert_eq!(
                (
                    &envelope["revision"],
                    &envelope["state"],
                    &envelope["sessionId"]
                ),
                (&json!(1), &json!("completed"), &json!(session))
            );
            let starts: Vec<_> = lifecycle
                .iter()
                .filter(|event| {
                    event["method"] == "item/started"
                        && event["params"]["item"]["id"] == envelope["id"]
                })
                .collect();
            assert_eq!(starts.len(), 1);
            assert_eq!(starts[0]["params"]["item"]["seq"], envelope["seq"]);
            prose_ids.push(envelope["id"].clone());
        }
        assert!(
            prose_ids[0] != prose_ids[1]
                && prose_ids[1] != prose_ids[2]
                && prose_ids[0] != prose_ids[2]
        );
        let before_end = events
            .iter()
            .position(|event| {
                event["method"] == "item/completed"
                    && event["params"]["item"]["item"]["text"] == "Before plan\n"
            })
            .context("before completion")?;
        let plan_start = events
            .iter()
            .position(|event| {
                event["method"] == "item/started"
                    && event["params"]["item"]["item"]["type"] == "plan"
            })
            .context("plan start")?;
        assert!(
            before_end < plan_start,
            "authoritative prose completion must precede plan boundary"
        );
        let plan_completed: Vec<_> = lifecycle
            .iter()
            .filter(|event| {
                event["method"] == "item/completed"
                    && event["params"]["item"]["item"]["type"] == "plan"
            })
            .collect();
        assert_eq!(plan_completed.len(), 1);
        assert_eq!(
            plan_completed[0]["params"]["item"]["item"]["entries"][0]["step"],
            "Plan body\n"
        );
        let reasoning: Vec<_> = lifecycle
            .iter()
            .filter(|event| {
                event["method"] == "item/completed"
                    && event["params"]["item"]["item"]["type"] == "reasoning"
            })
            .collect();
        assert_eq!(reasoning.len(), 1);
        assert_eq!(
            reasoning[0]["params"]["item"]["item"]["text"],
            "Plan reasoning"
        );
        let reasoning_start = events
            .iter()
            .position(|event| {
                event["method"] == "item/started"
                    && event["params"]["item"]["item"]["type"] == "reasoning"
            })
            .context("reasoning start")?;
        assert!(reasoning_start < before_end);
    }
    Ok(())
}
