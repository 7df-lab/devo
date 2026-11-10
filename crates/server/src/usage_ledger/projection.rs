//! Replayable accounting projection. Provider samples never enter this sum.
//! Records survive transcript edits/rollback: removing context does not refund spend.
use super::totals::{add_totals, record_totals};
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use devo_core::{InternalRecord, RolloutLine, SessionId, TurnId};
use devo_protocol::native::session::SessionParent;
use devo_protocol::native::usage::{
    PurposeUsage, SessionUsage, TurnUsage, UsageAttribution, UsageCallOutcome, UsageField,
    UsagePurpose, UsageRecord, UsageTotals,
};

mod legacy;
use legacy::{Candidate, Component, LegacyComponent};

#[derive(Default)]
pub(super) struct Projection {
    records: HashMap<String, UsageRecord>,
    sessions: HashSet<SessionId>,
    parents: HashMap<SessionId, SessionId>,
    legacy_candidates: HashMap<(SessionId, TurnId), Vec<Candidate>>,
    legacy_turns: HashMap<(SessionId, TurnId, Component), LegacyComponent>,
    turn_starts: HashMap<(SessionId, TurnId), DateTime<Utc>>,
    contexts: HashMap<SessionId, (DateTime<Utc>, TurnUsage)>,
    explicit_legacy: HashMap<SessionId, UsageTotals>,
    owners: HashMap<SessionId, UsageAttribution>,
    context_checkpoints: HashMap<SessionId, DateTime<Utc>>,
    fork_created_at: HashMap<SessionId, DateTime<Utc>>,
}

impl Projection {
    pub(super) fn observe(&mut self, line: RolloutLine) {
        match line {
            RolloutLine::SessionMeta { session, .. } => {
                self.sessions.insert(session.id);
                if session.fork_from_id.is_some() {
                    self.fork_created_at.insert(session.id, session.created_at);
                }
                if let Some(SessionParent::Agent { session_id, .. }) = session.parent {
                    self.parents.insert(session.id, session_id);
                }
                if let Some(legacy) = session.usage.legacy {
                    self.explicit_legacy.insert(session.id, legacy);
                }
            }
            RolloutLine::Turn {
                timestamp,
                turn,
                extras,
                ..
            } => {
                if extras
                    .as_ref()
                    .is_some_and(|extras| extras.context_occupancy.is_some())
                {
                    self.context_checkpoints
                        .entry(turn.session_id)
                        .and_modify(|time| *time = (*time).max(timestamp))
                        .or_insert(timestamp);
                }
                if let Some(usage) = extras.and_then(|extras| extras.latest_query_usage)
                    && (usage.query.metered_call_count > 0 || usage.display_total_tokens() > 0)
                    && usage
                        .query
                        .unreported_fields
                        .as_ref()
                        .map_or(usage.display_total_tokens() > 0, |fields| {
                            !fields.contains(&UsageField::TotalTokens)
                        })
                {
                    self.observe_context(turn.session_id, timestamp, usage);
                }
                self.sessions.insert(turn.session_id);
                self.turn_starts
                    .insert((turn.session_id, turn.id), turn.started_at);
                // A summary written before the first call record is a legacy
                // baseline. Later summaries overlap the records and must not
                // replace it (including a resumed pre-upgrade turn).
                if let Some(usage) = turn.usage
                    && !self
                        .fork_created_at
                        .get(&turn.session_id)
                        .is_some_and(|created| turn.started_at < *created)
                {
                    self.remember_legacy(
                        turn.session_id,
                        turn.id,
                        usage,
                        turn.started_at,
                        timestamp,
                    );
                }
            }

            RolloutLine::CompactionSnapshot {
                session_id,
                timestamp,
                ..
            } => {
                self.context_checkpoints
                    .entry(session_id)
                    .and_modify(|time| *time = (*time).max(timestamp))
                    .or_insert(timestamp);
            }
            RolloutLine::Internal {
                entry: InternalRecord::UsageRecord { record },
                ..
            } => {
                self.record(record);
            }
            _ => {}
        }
    }

    pub(super) fn register_session(&mut self, session_id: SessionId, parent: Option<SessionId>) {
        self.sessions.insert(session_id);
        if let Some(parent) = parent {
            self.parents.insert(session_id, parent);
        }
    }

    pub(super) fn register_owner(&mut self, child: SessionId, owner: UsageAttribution) {
        self.sessions.insert(child);
        self.sessions.insert(owner.session_id);
        self.parents.insert(child, owner.session_id);
        self.owners.insert(child, owner);
    }

    pub(super) fn owner_for_call(
        &self,
        session_id: SessionId,
        turn_id: Option<TurnId>,
    ) -> Option<UsageAttribution> {
        // A resumed/retried child turn keeps its recorded owner even if a new
        // message has since queued work for the same child under another turn.
        self.records
            .values()
            .find(|record| {
                turn_id.is_some()
                    && record.session_id == session_id
                    && record.turn_id == turn_id
                    && record.parent.is_some()
            })
            .and_then(|record| record.parent.clone())
            .or_else(|| self.owners.get(&session_id).cloned())
    }

    pub(super) fn record(&mut self, record: UsageRecord) {
        if self.records.contains_key(&record.call_id) {
            return;
        }
        self.sessions.insert(record.session_id);
        if let Some(parent) = &record.parent {
            self.parents.insert(record.session_id, parent.session_id);
        }
        if record.purpose == UsagePurpose::TurnQuery
            && record.outcome == UsageCallOutcome::Succeeded
            && record.usage.as_ref().is_some_and(|usage| {
                usage
                    .unreported_fields
                    .as_ref()
                    .is_some_and(|fields| !fields.contains(&UsageField::TotalTokens))
            })
            && !self.records.contains_key(&record.call_id)
        {
            self.observe_context(
                record.session_id,
                record.recorded_at,
                TurnUsage {
                    query: record_totals(&record),
                    overhead: UsageTotals::default(),
                },
            );
        }
        // The terminal record for an attempt is immutable, including unknown usage.
        let affected = self
            .legacy_candidates
            .keys()
            .copied()
            .filter(|(session, turn)| self.belongs_to_turn(&record, *session, *turn))
            .collect::<Vec<_>>();
        self.records.entry(record.call_id.clone()).or_insert(record);
        for (session, turn) in affected {
            self.refresh_legacy(session, turn);
        }
    }

    pub(super) fn ancestors(&self, session_id: SessionId) -> Vec<SessionId> {
        let mut result = vec![session_id];
        let mut current = session_id;
        while let Some(parent) = self.parents.get(&current).copied() {
            if result.contains(&parent) {
                break;
            }
            result.push(parent);
            current = parent;
        }
        result
    }

    fn includes(&self, session_id: SessionId, owner: SessionId) -> bool {
        self.ancestors(owner).contains(&session_id)
    }

    pub(super) fn session(&self, session_id: SessionId) -> Option<SessionUsage> {
        if !self.sessions.contains(&session_id) {
            return None;
        }
        let mut total = UsageTotals::default();
        let mut purposes = HashMap::<UsagePurpose, UsageTotals>::new();
        let mut updated_at = DateTime::<Utc>::UNIX_EPOCH;
        for record in self
            .records
            .values()
            .filter(|r| self.includes(session_id, r.session_id))
        {
            let usage = record_totals(record);
            add_totals(&mut total, &usage);
            add_totals(purposes.entry(record.purpose).or_default(), &usage);
            updated_at = updated_at.max(record.recorded_at);
        }
        let mut legacy = UsageTotals::default();
        let baselines = self
            .legacy_turns
            .iter()
            .filter(|((owner, _, component), baseline)| {
                self.includes(session_id, *owner)
                    && !self.legacy_turns.iter().any(
                        |((ancestor, _, covering_component), covering)| {
                            ancestor != owner
                                && component == covering_component
                                && self.includes(session_id, *ancestor)
                                && self.includes(*ancestor, *owner)
                                && baseline.started_at <= covering.recorded_at
                        },
                    )
            })
            .collect::<Vec<_>>();
        for (_, baseline) in &baselines {
            add_totals(&mut legacy, &baseline.totals);
        }
        // Historical ancestor summaries already included child spend. Split out
        // identifiable old child records from that opaque lump before adding
        // them as calls. Never add both the inclusive ancestor and child lump.
        for record in self
            .records
            .values()
            .filter(|r| self.includes(session_id, r.session_id))
        {
            if record.parent.is_none()
                && baselines.iter().any(|((owner, _, component), baseline)| {
                    *owner != record.session_id
                        && *component == Component::of(record.purpose)
                        && self.includes(*owner, record.session_id)
                        && (record.recorded_at <= baseline.recorded_at
                            || record
                                .turn_id
                                .and_then(|turn| self.turn_starts.get(&(record.session_id, turn)))
                                .is_some_and(|start| *start <= baseline.recorded_at))
                })
            {
                let covered = record_totals(record);
                legacy.total_tokens = legacy.total_tokens.saturating_sub(covered.total_tokens);
                legacy.input_tokens = legacy.input_tokens.saturating_sub(covered.input_tokens);
                legacy.output_tokens = legacy.output_tokens.saturating_sub(covered.output_tokens);
                legacy.reasoning_tokens = legacy
                    .reasoning_tokens
                    .saturating_sub(covered.reasoning_tokens);
                legacy.cache_read_input_tokens = legacy
                    .cache_read_input_tokens
                    .saturating_sub(covered.cache_read_input_tokens);
                legacy.cache_creation_input_tokens = legacy
                    .cache_creation_input_tokens
                    .saturating_sub(covered.cache_creation_input_tokens);
            }
        }
        // Explicit pre-call baselines are used only when no legacy turn details
        // exist for that session, never in addition to those same turn lumps.
        for (owner, usage) in &self.explicit_legacy {
            if self.includes(session_id, *owner)
                && !self.legacy_turns.keys().any(|(id, _, _)| id == owner)
            {
                let mut usage = usage.clone();
                if usage.unreported_fields.is_none() {
                    usage.unreported_fields = Some(
                        UsageField::ALL
                            .into_iter()
                            .filter(|field| *field != UsageField::TotalTokens)
                            .collect(),
                    );
                }
                add_totals(&mut legacy, &usage);
            }
        }
        let legacy = (legacy != UsageTotals::default()).then_some(legacy);
        if let Some(legacy) = &legacy {
            add_totals(&mut total, legacy);
        }
        let by_purpose = [
            UsagePurpose::TurnQuery,
            UsagePurpose::AutoReview,
            UsagePurpose::Compaction,
            UsagePurpose::TitleGeneration,
            UsagePurpose::Refine,
            UsagePurpose::PythonCellWatch,
        ]
        .into_iter()
        .filter_map(|purpose| {
            let usage = purposes.remove(&purpose)?;
            Some(PurposeUsage {
                unreported_fields: usage.unreported_fields.clone(),
                purpose,
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                reasoning_tokens: usage.reasoning_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                call_count: usage.call_count,
                metered_call_count: usage.metered_call_count,
                failed_call_count: usage.failed_call_count,
                cancelled_call_count: usage.cancelled_call_count,
                estimated_cost: usage.estimated_cost,
            })
        })
        .collect();
        Some(SessionUsage {
            total,
            by_purpose,
            legacy,
            updated_at,
        })
    }

    pub(super) fn turn(&self, session_id: SessionId, turn_id: TurnId) -> TurnUsage {
        let mut result = TurnUsage {
            query: self
                .legacy_turns
                .get(&(session_id, turn_id, Component::Query))
                .map(|legacy| legacy.totals.clone())
                .unwrap_or_default(),
            overhead: self
                .legacy_turns
                .get(&(session_id, turn_id, Component::Overhead))
                .map(|legacy| legacy.totals.clone())
                .unwrap_or_default(),
        };
        for record in self.records.values() {
            if self.belongs_to_turn(record, session_id, turn_id) {
                let target = if record.purpose == UsagePurpose::TurnQuery {
                    &mut result.query
                } else {
                    &mut result.overhead
                };
                add_totals(target, &record_totals(record));
            }
        }
        result
    }

    fn belongs_to_turn(
        &self,
        record: &UsageRecord,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> bool {
        if record.session_id == session_id && record.turn_id == Some(turn_id) {
            return true;
        }
        let mut parent = record.parent.as_ref();
        let mut visited = HashSet::new();
        while let Some(owner) = parent {
            if !visited.insert(owner.session_id) {
                break;
            }
            if owner.session_id == session_id && owner.turn_id == Some(turn_id) {
                return true;
            }
            parent = self
                .records
                .values()
                .find(|r| {
                    r.session_id == owner.session_id
                        && r.turn_id == owner.turn_id
                        && r.parent.is_some()
                })
                .and_then(|r| r.parent.as_ref());
        }
        false
    }

    fn observe_context(
        &mut self,
        session_id: SessionId,
        timestamp: DateTime<Utc>,
        usage: TurnUsage,
    ) {
        if self
            .contexts
            .get(&session_id)
            .is_none_or(|(previous, _)| *previous <= timestamp)
        {
            self.contexts.insert(session_id, (timestamp, usage));
        }
    }

    /// Only this logical session's query calls can anchor its context.
    pub(super) fn query_after_context_checkpoint(&self, session_id: SessionId) -> bool {
        self.records.values().any(|record| {
            record.session_id == session_id
                && record.purpose == UsagePurpose::TurnQuery
                && record.outcome == UsageCallOutcome::Succeeded
                && record.usage.as_ref().is_some_and(|usage| {
                    usage
                        .unreported_fields
                        .as_ref()
                        .is_some_and(|fields| !fields.contains(&UsageField::TotalTokens))
                })
                && self
                    .context_checkpoints
                    .get(&session_id)
                    .is_none_or(|checkpoint| record.recorded_at > *checkpoint)
        })
    }

    pub(super) fn latest_input(&self, session_id: SessionId) -> Option<u64> {
        self.records
            .values()
            .filter(|record| {
                record.session_id == session_id
                    && record.purpose == UsagePurpose::TurnQuery
                    && record.outcome == UsageCallOutcome::Succeeded
                    && record.usage.as_ref().is_some_and(|usage| {
                        usage
                            .unreported_fields
                            .as_ref()
                            .is_some_and(|fields| !fields.contains(&UsageField::InputTokens))
                    })
            })
            .max_by_key(|record| record.recorded_at)
            .and_then(|record| record.usage.as_ref().map(|usage| usage.input_tokens))
    }

    pub(super) fn latest_query(&self, session_id: SessionId) -> Option<TurnUsage> {
        self.contexts
            .get(&session_id)
            .map(|(_, usage)| usage.clone())
    }
}
