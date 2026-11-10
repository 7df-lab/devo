//! Legacy summaries are candidate checkpoints, not a second accumulator. Select
//! the last pre-record component after considering all logical journal streams.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Component {
    Query,
    Overhead,
}
impl Component {
    pub(super) fn of(purpose: UsagePurpose) -> Self {
        if purpose == UsagePurpose::TurnQuery {
            Self::Query
        } else {
            Self::Overhead
        }
    }
}

#[derive(PartialEq)]
pub(super) struct Candidate {
    usage: TurnUsage,
    started_at: DateTime<Utc>,
    recorded_at: DateTime<Utc>,
}

pub(super) struct LegacyComponent {
    pub(super) totals: UsageTotals,
    pub(super) started_at: DateTime<Utc>,
    pub(super) recorded_at: DateTime<Utc>,
}

impl Projection {
    pub(super) fn remember_legacy(
        &mut self,
        session: SessionId,
        turn: TurnId,
        mut usage: TurnUsage,
        started_at: DateTime<Utc>,
        recorded_at: DateTime<Utc>,
    ) {
        for totals in [&mut usage.query, &mut usage.overhead] {
            if *totals != UsageTotals::default() {
                totals.unreported_fields = Some(
                    UsageField::ALL
                        .into_iter()
                        .filter(|field| *field != UsageField::TotalTokens)
                        .collect(),
                );
            }
            totals.call_count = 0;
            totals.metered_call_count = 0;
            totals.failed_call_count = 0;
            totals.cancelled_call_count = 0;
        }
        if usage == TurnUsage::default() {
            return;
        }
        let candidate = Candidate {
            usage,
            started_at,
            recorded_at,
        };
        let candidates = self.legacy_candidates.entry((session, turn)).or_default();
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
        self.refresh_legacy(session, turn);
    }

    pub(super) fn refresh_legacy(&mut self, session: SessionId, turn: TurnId) {
        for component in [Component::Query, Component::Overhead] {
            let first_record = self
                .records
                .values()
                .filter(|record| {
                    Component::of(record.purpose) == component
                        && self.belongs_to_turn(record, session, turn)
                })
                .map(|record| record.recorded_at)
                .min();
            let selected = self
                .legacy_candidates
                .get(&(session, turn))
                .into_iter()
                .flatten()
                .filter(|candidate| first_record.is_none_or(|first| candidate.recorded_at < first))
                .max_by_key(|candidate| candidate.recorded_at)
                .map(|candidate| LegacyComponent {
                    totals: match component {
                        Component::Query => candidate.usage.query.clone(),
                        Component::Overhead => candidate.usage.overhead.clone(),
                    },
                    started_at: candidate.started_at,
                    recorded_at: candidate.recorded_at,
                })
                .filter(|baseline| baseline.totals != UsageTotals::default());
            let key = (session, turn, component);
            if let Some(selected) = selected {
                self.legacy_turns.insert(key, selected);
            } else {
                self.legacy_turns.remove(&key);
            }
        }
    }
}
