mod journal;
mod projection;
mod totals;

use chrono::Utc;
use devo_core::SessionId;
use devo_core::TurnId;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_protocol::native::model::ModelBinding;
use devo_protocol::native::usage::{
    CallContext, SessionUsage, TokenUsage, TurnUsage, UsageAttribution, UsageCallOutcome,
    UsagePurpose, UsageRecord,
};
use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
use tracing::warn;
use uuid::Uuid;

use anyhow::Result;
use async_trait::async_trait;
use devo_provider::ModelProviderSDK;
use futures::Stream;

use crate::db::Database;
use crate::persistence::RolloutStore;

#[derive(Clone)]
pub(crate) struct UsageLedger {
    rollout_store: RolloutStore,
    db: Arc<Database>,
    state: Arc<std::sync::Mutex<LedgerState>>,
    updates: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<SessionId>>>>,
}

#[derive(Default)]
struct LedgerState {
    projection: projection::Projection,
    ephemeral: std::collections::HashSet<SessionId>,
    loaded_paths: std::collections::HashMap<std::path::PathBuf, journal::JournalCursor>,
}

impl UsageLedger {
    pub(crate) fn new(rollout_store: RolloutStore, db: Arc<Database>) -> Self {
        Self {
            rollout_store,
            db,
            state: Default::default(),
            updates: Default::default(),
        }
    }

    pub(crate) fn instrumented_provider(
        &self,
        provider: Arc<dyn ModelProviderSDK>,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        purpose: UsagePurpose,
    ) -> Arc<dyn ModelProviderSDK> {
        Arc::new(InstrumentedProvider {
            provider,
            ledger: self.clone(),
            context: RuntimeCallContext::new(session_id, turn_id, purpose),
        })
    }

    pub(crate) fn set_updates(&self, sender: tokio::sync::mpsc::UnboundedSender<SessionId>) {
        *self.updates.lock().expect("usage updates lock") = Some(sender);
    }

    /// Reuse unchanged journals, and replay appended records after another process
    /// releases its writer lease. Shared journals provide all descendant
    /// records; the index also finds pre-upgrade independent child journals.
    pub(crate) fn load_session(&self, session_id: SessionId) -> Result<()> {
        let mut state = self.state.lock().expect("usage ledger lock");
        let rows = self.db.list_sessions()?;
        let mut ids = std::collections::HashSet::from([session_id]);
        // Include the actual agent ancestors and descendants, never user forks.
        loop {
            let count = ids.len();
            for row in &rows {
                if row.agent_path.is_some()
                    && let Some(parent) = row.parent_session_id
                    && (ids.contains(&parent) || ids.contains(&row.session_id))
                {
                    ids.insert(parent);
                    ids.insert(row.session_id);
                }
            }
            if ids.len() == count {
                break;
            }
        }
        for id in ids {
            let Some(index) = self.db.get_session_index(&id)? else {
                continue;
            };
            // New child metadata may be appended to an already-loaded root file.
            let parent = index
                .session
                .clone()
                .into_native_session()
                .parent
                .map(|parent| {
                    let devo_protocol::native::session::SessionParent::Agent { session_id, .. } =
                        parent;
                    session_id
                });
            state
                .projection
                .register_session(id, parent, index.session.updated_at);
            let Some(path) = index.rollout_path else {
                continue;
            };
            let mut cursor = state.loaded_paths.remove(&path).unwrap_or_default();
            let replay = cursor.read_new(&path, |line| state.projection.observe(line));
            state.loaded_paths.insert(path, cursor);
            replay?;
        }
        Ok(())
    }

    pub(crate) fn initialize_session(
        &self,
        summary: &crate::runtime_session_summary::RuntimeSessionSummary,
    ) -> Result<()> {
        self.load_session(summary.session_id())?;
        let mut state = self.state.lock().expect("usage ledger lock");
        state.projection.register_session(
            summary.session_id(),
            summary.parent_session_id(),
            summary.native.usage.updated_at,
        );
        if summary.ephemeral {
            state.ephemeral.insert(summary.session_id());
        }
        Ok(())
    }

    pub(crate) fn register_owner(&self, child: SessionId, owner: UsageAttribution) {
        self.state
            .lock()
            .expect("usage ledger lock")
            .projection
            .register_owner(child, owner);
    }

    pub(crate) fn session_usage(&self, session_id: SessionId) -> Option<SessionUsage> {
        self.state
            .lock()
            .expect("usage ledger lock")
            .projection
            .session(session_id)
    }

    pub(crate) fn turn_usage(&self, session_id: SessionId, turn_id: TurnId) -> TurnUsage {
        self.state
            .lock()
            .expect("usage ledger lock")
            .projection
            .turn(session_id, turn_id)
    }

    pub(crate) fn query_after_context_checkpoint(&self, session_id: SessionId) -> bool {
        self.state
            .lock()
            .expect("usage ledger lock")
            .projection
            .query_after_context_checkpoint(session_id)
    }

    pub(crate) fn latest_input(&self, session_id: SessionId) -> Option<u64> {
        self.state
            .lock()
            .expect("usage ledger lock")
            .projection
            .latest_input(session_id)
    }

    pub(crate) fn latest_query(&self, session_id: SessionId) -> Option<TurnUsage> {
        self.state
            .lock()
            .expect("usage ledger lock")
            .projection
            .latest_query(session_id)
    }

    pub(crate) fn affected_sessions(&self, session_id: SessionId) -> Vec<SessionId> {
        self.state
            .lock()
            .expect("usage ledger lock")
            .projection
            .ancestors(session_id)
    }

    fn record(
        &self,
        call_id: &str,
        context: &RuntimeCallContext,
        provider: &str,
        request: &ModelRequest,
        outcome: UsageCallOutcome,
        usage: Option<&Usage>,
    ) -> Result<()> {
        self.load_session(context.session_id)?;
        let record = UsageRecord {
            call_id: call_id.to_owned(),
            session_id: context.call.session_id,
            turn_id: context.call.turn_id,
            parent: context.parent.clone(),
            purpose: context.call.purpose,
            model: ModelBinding {
                provider: provider.to_owned(),
                model: request.model.clone(),
                variant: None,
                reasoning_effort: request.reasoning_effort,
            },
            outcome,
            usage: usage.filter(|usage| usage.is_reported()).map(token_usage),
            estimated_cost: None,
            recorded_at: Utc::now(),
        };
        let index = self.db.get_session_index(&context.session_id)?;
        if let Some(path) = index.as_ref().and_then(|index| index.rollout_path.as_ref()) {
            self.rollout_store
                .append_usage_record(path, context.session_id, record.clone())?;
        } else if !index.is_some_and(|index| index.session.ephemeral)
            && !self
                .state
                .lock()
                .expect("usage ledger lock")
                .ephemeral
                .contains(&context.session_id)
        {
            anyhow::bail!(
                "cannot persist model-call usage: session {} has no journal",
                context.session_id
            );
        }
        self.state
            .lock()
            .expect("usage ledger lock")
            .projection
            .record(record);
        if let Some(sender) = self.updates.lock().expect("usage updates lock").as_ref() {
            let _ = sender.send(context.session_id);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct RuntimeCallContext {
    session_id: SessionId,
    call: CallContext,
    parent: Option<UsageAttribution>,
}

impl RuntimeCallContext {
    fn new(session_id: SessionId, turn_id: Option<TurnId>, purpose: UsagePurpose) -> Self {
        Self {
            session_id,
            parent: None,
            call: CallContext {
                session_id,
                turn_id,
                purpose,
            },
        }
    }
}

fn token_usage(usage: &Usage) -> TokenUsage {
    TokenUsage {
        unreported_fields: usage.unreported_fields.clone(),
        total_tokens: usage.total_tokens.map(|total| total as u64),
        input_tokens: usage.input_tokens as u64,
        output_tokens: usage.output_tokens as u64,
        reasoning_tokens: usage.reasoning_output_tokens.unwrap_or(0) as u64,
        cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0) as u64,
        cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0) as u64,
    }
}

struct InstrumentedProvider {
    provider: Arc<dyn ModelProviderSDK>,
    ledger: UsageLedger,
    context: RuntimeCallContext,
}

#[async_trait]
impl ModelProviderSDK for InstrumentedProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut attempt = CallAttemptGuard::new(
            self.ledger.clone(),
            self.context.clone(),
            self.provider.name().to_owned(),
            request.clone(),
        );
        let samples = attempt.last_usage.clone();
        let observer = Arc::new(move |usage: &Usage| {
            if usage.is_reported() {
                *samples.lock().expect("usage sample lock") = Some(usage.clone());
            }
        });
        match self
            .provider
            .completion_with_usage_observer(request, observer)
            .await
        {
            Ok(mut response) => {
                let usage = if response.usage.is_reported() {
                    Some(response.usage.clone())
                } else {
                    attempt
                        .last_usage
                        .lock()
                        .expect("usage sample lock")
                        .clone()
                };
                attempt.finish(UsageCallOutcome::Succeeded, usage.as_ref())?;
                if let Some(usage) = usage {
                    response.usage = usage;
                }
                Ok(response)
            }
            Err(error) => {
                let usage = attempt
                    .last_usage
                    .lock()
                    .expect("usage sample lock")
                    .clone();
                attempt.finish(UsageCallOutcome::Failed, usage.as_ref())?;
                Err(error)
            }
        }
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let mut attempt = CallAttemptGuard::new(
            self.ledger.clone(),
            self.context.clone(),
            self.provider.name().to_owned(),
            request.clone(),
        );
        match self.provider.completion_stream(request.clone()).await {
            Ok(inner) => Ok(Box::pin(MeteredStream { inner, attempt })),
            Err(error) => {
                attempt.finish(UsageCallOutcome::Failed, None)?;
                Err(error)
            }
        }
    }

    fn name(&self) -> &str {
        self.provider.name()
    }
}

struct CallAttemptGuard {
    ledger: UsageLedger,
    context: RuntimeCallContext,
    provider_name: String,
    request: ModelRequest,
    terminal: bool,
    call_id: String,
    last_usage: Arc<std::sync::Mutex<Option<Usage>>>,
}

impl CallAttemptGuard {
    fn new(
        ledger: UsageLedger,
        context: RuntimeCallContext,
        provider_name: String,
        request: ModelRequest,
    ) -> Self {
        let mut context = context;
        context.parent = ledger
            .state
            .lock()
            .expect("usage ledger lock")
            .projection
            .owner_for_call(context.session_id, context.call.turn_id);
        Self {
            ledger,
            context,
            provider_name,
            request,
            terminal: false,
            call_id: Uuid::now_v7().to_string(),
            last_usage: Default::default(),
        }
    }

    fn finish(&mut self, outcome: UsageCallOutcome, usage: Option<&Usage>) -> Result<()> {
        if self.terminal {
            return Ok(());
        }
        self.terminal = true;
        self.ledger.record(
            &self.call_id,
            &self.context,
            &self.provider_name,
            &self.request,
            outcome,
            usage,
        )
    }
}

impl Drop for CallAttemptGuard {
    fn drop(&mut self) {
        let usage = self.last_usage.lock().expect("usage sample lock").take();
        if let Err(error) = self.finish(UsageCallOutcome::Cancelled, usage.as_ref()) {
            warn!(%error, "failed to persist cancelled model-call usage");
        }
    }
}

struct MeteredStream {
    inner: Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>,
    attempt: CallAttemptGuard,
}

impl Stream for MeteredStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut item = self.inner.as_mut().poll_next(context);
        if let Poll::Ready(Some(Ok(StreamEvent::MessageDone { response }))) = &mut item
            && !response.usage.is_reported()
            && let Some(usage) = self
                .attempt
                .last_usage
                .lock()
                .expect("usage sample lock")
                .clone()
        {
            // The reliable terminal event must carry the last real sample too;
            // otherwise losing its best-effort delta loses the context anchor.
            response.usage = usage;
        }
        let result = match &item {
            Poll::Ready(Some(Ok(StreamEvent::UsageDelta(usage)))) if usage.is_reported() => {
                *self.attempt.last_usage.lock().expect("usage sample lock") = Some(usage.clone());
                Ok(())
            }
            Poll::Ready(Some(Ok(StreamEvent::MessageDone { response }))) => {
                let usage = if response.usage.is_reported() {
                    Some(response.usage.clone())
                } else {
                    self.attempt
                        .last_usage
                        .lock()
                        .expect("usage sample lock")
                        .clone()
                };
                self.attempt
                    .finish(UsageCallOutcome::Succeeded, usage.as_ref())
            }
            Poll::Ready(Some(Err(_))) | Poll::Ready(None) => {
                let usage = self
                    .attempt
                    .last_usage
                    .lock()
                    .expect("usage sample lock")
                    .take();
                self.attempt
                    .finish(UsageCallOutcome::Failed, usage.as_ref())
            }
            Poll::Pending | Poll::Ready(Some(Ok(_))) => Ok(()),
        };
        if let Err(error) = result {
            return Poll::Ready(Some(Err(error)));
        }
        item
    }
}

#[cfg(test)]
mod tests;
