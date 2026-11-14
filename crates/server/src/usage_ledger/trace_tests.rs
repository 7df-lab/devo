use super::*;
use pretty_assertions::assert_eq;
use std::io::Write;
use std::sync::Mutex;
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct LoggingProvider;

#[async_trait]
impl ModelProviderSDK for LoggingProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        tokio::task::yield_now().await;
        tracing::debug!("completion provider probe");
        if request.model == "failed-model" {
            anyhow::bail!("fixture failure");
        }
        Ok(response(reported(2, 1)))
    }

    async fn completion_stream(
        &self,
        _: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        tokio::task::yield_now().await;
        tracing::debug!("stream handshake probe");
        Ok(Box::pin(stream::once(async {
            tokio::task::yield_now().await;
            tracing::debug!("stream poll probe");
            Ok(StreamEvent::MessageDone {
                response: response(reported(3, 1)),
            })
        })))
    }

    fn name(&self) -> &str {
        "logging-provider"
    }
}

#[tokio::test]
async fn interleaved_calls_link_provider_logs_to_journal_without_leaking_context() {
    let fixture = Fixture::new();
    let first_turn = TurnId::new();
    let second_turn = TurnId::new();
    let parent = UsageAttribution {
        session_id: SessionId::new(),
        turn_id: Some(TurnId::new()),
    };
    fixture
        .ledger
        .register_owner(fixture.session.id, parent.clone());
    let completion = fixture.ledger.instrumented_provider(
        Arc::new(LoggingProvider),
        fixture.session.id,
        Some(first_turn),
        UsagePurpose::Compaction,
    );
    let streaming = fixture.ledger.instrumented_provider(
        Arc::new(LoggingProvider),
        fixture.session.id,
        Some(second_turn),
        UsagePurpose::TurnQuery,
    );
    let buffer = LogBuffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .finish();
    let mut sensitive = request();
    sensitive.system = Some("PRIVATE-PROMPT-MARKER".into());
    sensitive.extra_body = Some(serde_json::json!({"api_key": "PRIVATE-KEY-MARKER"}));
    let mut streaming_request = sensitive.clone();
    sensitive.model = "compaction-model".into();
    streaming_request.model = "query-model".into();
    async {
        let (completed, streamed) = tokio::join!(
            completion.completion(sensitive.clone()),
            streaming.completion_stream(streaming_request)
        );
        completed.unwrap();
        fixture.ledger.register_owner(
            fixture.session.id,
            UsageAttribution {
                session_id: parent.session_id,
                turn_id: Some(TurnId::new()),
            },
        );
        let mut streamed = streamed.unwrap();
        streamed.next().await.unwrap().unwrap();
        drop(streamed);
        tracing::debug!("outside call probe");
    }
    .with_subscriber(subscriber)
    .await;

    let raw = std::fs::read_to_string(&fixture.session.rollout_path).unwrap();
    let records: Vec<_> = raw
        .lines()
        .filter_map(|line| match parse_rollout_line(line).unwrap() {
            RolloutLine::Internal {
                entry: devo_core::InternalRecord::UsageRecord { record },
                ..
            } => Some(record),
            _ => None,
        })
        .collect();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0].call_id, records[1].call_id);
    let logs = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    for record in &records {
        assert_eq!(record.parent, Some(parent.clone()));
        let probes: &[&str] = match record.purpose {
            UsagePurpose::Compaction => &["completion provider probe"],
            UsagePurpose::TurnQuery => &["stream handshake probe", "stream poll probe"],
            _ => panic!("unexpected call purpose"),
        };
        for probe in probes {
            let line = logs.lines().find(|line| line.contains(probe)).unwrap();
            assert!(line.contains(&record.call_id), "{line}");
            assert!(line.contains(&record.session_id.to_string()), "{line}");
            assert!(line.contains(&record.model.model), "{line}");
            assert!(
                line.contains(&record.turn_id.unwrap().to_string()),
                "{line}"
            );
            let other = records
                .iter()
                .find(|other| other.call_id != record.call_id)
                .unwrap();
            assert!(!line.contains(&other.call_id), "{line}");
        }
    }
    let outside = logs
        .lines()
        .find(|line| line.contains("outside call probe"))
        .unwrap();
    assert!(!outside.contains("model_call"), "{outside}");
    assert!(!logs.contains("PRIVATE-PROMPT-MARKER"));
    assert!(!logs.contains("PRIVATE-KEY-MARKER"));
}

#[tokio::test]
async fn compaction_diagnostics_do_not_copy_prompt_or_summary_text() {
    use devo_core::history::compaction::HistorySummarizer;
    use devo_core::history::summarizer::DefaultHistorySummarizer;
    use devo_protocol::{RequestContent, RequestMessage};

    let buffer = LogBuffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .finish();
    let summarizer = DefaultHistorySummarizer::with_models(
        Arc::new(LoggingProvider),
        "test",
        "test",
        /*max_tokens*/ 128,
    );
    let summary = summarizer
        .summarize(
            vec![RequestMessage {
                role: "user".into(),
                content: vec![RequestContent::Text {
                    text: "PRIVATE-COMPACTION-PROMPT".into(),
                }],
            }],
            /*cancel_token*/ None,
        )
        .with_subscriber(subscriber)
        .await
        .unwrap();
    assert_eq!(summary, "done");
    let logs = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("sending LLM compaction request"));
    assert!(logs.contains("received LLM compaction response"));
    assert!(!logs.contains("PRIVATE-COMPACTION-PROMPT"));
    assert!(!logs.contains("done"));
}

#[tokio::test]
async fn failed_and_cancelled_calls_keep_the_same_terminal_identity() {
    let fixture = Fixture::new();
    let provider = fixture.ledger.instrumented_provider(
        Arc::new(LoggingProvider),
        fixture.session.id,
        /*turn_id*/ None,
        UsagePurpose::TitleGeneration,
    );
    let buffer = LogBuffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .finish();
    async {
        let mut failed = request();
        failed.model = "failed-model".into();
        assert!(provider.completion(failed).await.is_err());
        let stream = provider.completion_stream(request()).await.unwrap();
        drop(stream);
    }
    .with_subscriber(subscriber)
    .await;
    let raw = std::fs::read_to_string(&fixture.session.rollout_path).unwrap();
    let records: Vec<_> = raw
        .lines()
        .filter_map(|line| match parse_rollout_line(line).unwrap() {
            RolloutLine::Internal {
                entry: devo_core::InternalRecord::UsageRecord { record },
                ..
            } => Some(record),
            _ => None,
        })
        .collect();
    assert_eq!(
        records
            .iter()
            .map(|record| (record.outcome, record.turn_id))
            .collect::<Vec<_>>(),
        vec![
            (UsageCallOutcome::Failed, None),
            (UsageCallOutcome::Cancelled, None)
        ],
    );
    let logs = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    for record in records {
        let terminal: Vec<_> = logs
            .lines()
            .filter(|line| line.contains("model call finished") && line.contains(&record.call_id))
            .collect();
        assert_eq!(terminal.len(), 1);
        assert!(terminal[0].contains(&format!("{:?}", record.outcome)));
    }
}
