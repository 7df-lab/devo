//! Persist schedule jobs under `$DEVO_HOME/schedules.json` and claimed deliveries
//! in the compatible `schedule-outbox.json` sidecar.
//!
//! The store is shared across runtime tasks via mutexes; wake/dispatch runs
//! outside the session actor mailbox (L2-DES-SERVER-002).

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tokio::sync::Mutex as AsyncMutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use devo_protocol::native::ids::{JobId, SessionId};
use devo_protocol::native::rpc_schedule::{
    ScheduleJob, ScheduleJobKind, ScheduleJobStatus, ScheduleUpdateAction,
    SessionScheduleUpsertParams,
};

mod outbox;
pub(crate) use outbox::ScheduleOccurrence;

const SCHEDULES_FILE_NAME: &str = "schedules.json";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct SchedulesFile {
    #[serde(default)]
    jobs: Vec<ScheduleJob>,
}

#[derive(Debug, Clone, Copy)]
enum DueClaimMode {
    #[cfg(test)]
    WithoutOutbox,
    WithOutbox,
}

pub(crate) struct ScheduleStore {
    path: PathBuf,
    lock: Mutex<()>,
    dispatch_lock: AsyncMutex<()>,
    inflight_occurrences: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl ScheduleStore {
    pub(crate) fn new(devo_home: impl Into<PathBuf>) -> Self {
        let path = devo_home.into().join(SCHEDULES_FILE_NAME);
        Self {
            path,
            lock: Mutex::new(()),
            dispatch_lock: AsyncMutex::new(()),
            inflight_occurrences: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn lock_dispatch(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.dispatch_lock.lock().await
    }

    pub(crate) fn list(
        &self,
        session_id: Option<&SessionId>,
        cwd: Option<&Path>,
    ) -> Result<Vec<ScheduleJob>> {
        self.list_filtered(session_id, cwd, /*include_stopped*/ false)
    }

    /// Like [`Self::list`], optionally keeping [`ScheduleJobStatus::Stopped`] jobs.
    pub(crate) fn list_filtered(
        &self,
        session_id: Option<&SessionId>,
        cwd: Option<&Path>,
        include_stopped: bool,
    ) -> Result<Vec<ScheduleJob>> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let jobs = self.read_unlocked()?.jobs;
        Ok(jobs
            .into_iter()
            .filter(|job| {
                if !include_stopped && matches!(job.status, ScheduleJobStatus::Stopped) {
                    return false;
                }
                if let Some(session_id) = session_id
                    && &job.session_id != session_id
                {
                    return false;
                }
                if let Some(cwd) = cwd {
                    match job.cwd.as_deref() {
                        Some(job_cwd) => job_cwd == cwd,
                        None => true,
                    }
                } else {
                    true
                }
            })
            .collect())
    }

    pub(crate) fn get_job(&self, job_id: &JobId) -> Result<Option<ScheduleJob>> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        Ok(self
            .read_unlocked()?
            .jobs
            .into_iter()
            .find(|job| &job.job_id == job_id))
    }

    pub(crate) fn upsert(&self, params: SessionScheduleUpsertParams) -> Result<ScheduleJob> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let mut file = self.read_unlocked()?;
        let now = Utc::now();
        let text = params
            .prompt
            .clone()
            .or_else(|| params.instruction.clone())
            .unwrap_or_default();
        let interval_ms = params
            .interval_ms
            .or_else(|| params.schedule.as_deref().and_then(parse_interval_ms))
            .filter(|ms| *ms > 0);
        // The dispatch loop only runs interval-based jobs (`next_run_at` is
        // derived exclusively from the interval). An unparsable or zero
        // schedule — e.g. a real cron expression like "0 9 * * *" — would
        // otherwise store an Active job that silently never fires.
        if interval_ms.is_none() {
            anyhow::bail!(
                "invalid schedule: {:?} cannot be parsed into a positive interval \
                 (expected forms like 'every 5m', '40s', '2h')",
                params.schedule
            );
        }
        let next_run_at = Some(
            next_run_from_interval(interval_ms, now)
                .context("invalid schedule: interval is outside the supported date range")?,
        );

        let job = if let Some(job_id) = params.job_id {
            let Some(existing) = file.jobs.iter_mut().find(|job| job.job_id == job_id) else {
                anyhow::bail!("job not found: {}", job_id.as_str());
            };
            existing.kind = params.kind;
            existing.session_id = params.session_id;
            existing.cwd = params.cwd;
            existing.schedule = params.schedule;
            existing.interval_ms = interval_ms;
            match params.kind {
                ScheduleJobKind::Cron => {
                    existing.prompt = Some(text.clone()).filter(|s| !s.is_empty());
                    existing.instruction = None;
                }
                ScheduleJobKind::Heartbeat => {
                    existing.instruction = Some(text.clone()).filter(|s| !s.is_empty());
                    existing.prompt = None;
                }
            }
            existing.delivery_mode = params.delivery_mode;
            existing.label = params.label;
            existing.updated_at = now;
            if existing.status == ScheduleJobStatus::Active {
                existing.next_run_at = next_run_at;
            }
            existing.clone()
        } else if params.kind == ScheduleJobKind::Heartbeat {
            // User `/heartbeat` is one-per-session (no label): replace the latest
            // active/paused heartbeat. Labeled RLM heartbeats may coexist.
            let replace_unlabeled = params.label.as_ref().is_none_or(|l| l.trim().is_empty());
            if replace_unlabeled
                && let Some(existing) = file.jobs.iter_mut().rev().find(|job| {
                    job.kind == ScheduleJobKind::Heartbeat
                        && job.session_id == params.session_id
                        && job.label.as_ref().is_none_or(|l| l.trim().is_empty())
                        && matches!(
                            job.status,
                            ScheduleJobStatus::Active | ScheduleJobStatus::Paused
                        )
                })
            {
                existing.cwd = params.cwd;
                existing.schedule = params.schedule;
                existing.interval_ms = interval_ms;
                existing.instruction = Some(text.clone()).filter(|s| !s.is_empty());
                existing.prompt = None;
                existing.delivery_mode = params.delivery_mode;
                existing.label = params.label;
                existing.status = ScheduleJobStatus::Active;
                existing.updated_at = now;
                existing.next_run_at = next_run_at;
                existing.clone()
            } else {
                let job = new_job(params, text, interval_ms, next_run_at, now);
                file.jobs.push(job.clone());
                job
            }
        } else {
            let job = new_job(params, text, interval_ms, next_run_at, now);
            file.jobs.push(job.clone());
            job
        };

        self.write_unlocked(&file)?;
        Ok(job)
    }

    pub(crate) fn update(
        &self,
        job_id: &JobId,
        action: ScheduleUpdateAction,
    ) -> Result<ScheduleJob> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let mut file = self.read_unlocked()?;
        let Some(job) = file.jobs.iter_mut().find(|job| &job.job_id == job_id) else {
            anyhow::bail!("job not found: {}", job_id.as_str());
        };
        if job.status == ScheduleJobStatus::Stopped && action != ScheduleUpdateAction::Stop {
            anyhow::bail!("stopped schedule is terminal");
        }
        let now = Utc::now();
        match action {
            ScheduleUpdateAction::Pause => {
                job.status = ScheduleJobStatus::Paused;
                job.next_run_at = None;
            }
            ScheduleUpdateAction::Resume => {
                let next_run_at = next_run_from_interval(job.interval_ms, now)
                    .context("invalid schedule: interval is outside the supported date range")?;
                job.status = ScheduleJobStatus::Active;
                job.next_run_at = Some(next_run_at);
            }
            ScheduleUpdateAction::Stop => {
                job.status = ScheduleJobStatus::Stopped;
                job.next_run_at = None;
            }
        }
        job.updated_at = now;
        let updated = job.clone();
        self.write_unlocked(&file)?;
        Ok(updated)
    }

    #[cfg(test)]
    pub(crate) fn retry_claimed(
        &self,
        claimed: &ScheduleJob,
        retry_at: DateTime<Utc>,
    ) -> Result<()> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let mut file = self.read_unlocked()?;
        let Some(job) = file
            .jobs
            .iter_mut()
            .find(|job| job.job_id == claimed.job_id)
        else {
            return Ok(());
        };
        if job.status != ScheduleJobStatus::Active
            || job.last_run_at != claimed.last_run_at
            || job.updated_at != claimed.updated_at
            || !job
                .next_run_at
                .is_none_or(|next_run_at| next_run_at > retry_at)
        {
            return Ok(());
        }
        job.next_run_at = Some(retry_at);
        job.updated_at = Utc::now();
        self.write_unlocked(&file)
    }

    pub(crate) fn delete(&self, job_id: &JobId) -> Result<ScheduleJob> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let mut file = self.read_unlocked()?;
        let Some(index) = file.jobs.iter().position(|job| &job.job_id == job_id) else {
            anyhow::bail!("job not found: {}", job_id.as_str());
        };
        let mut job = file.jobs.remove(index);
        job.status = ScheduleJobStatus::Stopped;
        job.updated_at = Utc::now();
        job.next_run_at = None;
        self.write_unlocked(&file)?;
        Ok(job)
    }

    #[cfg(test)]
    pub(crate) fn claim_due(&self, now: DateTime<Utc>) -> Result<Vec<ScheduleJob>> {
        self.claim_due_inner(now, DueClaimMode::WithoutOutbox)
    }

    fn claim_due_inner(&self, now: DateTime<Utc>, mode: DueClaimMode) -> Result<Vec<ScheduleJob>> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let mut file = self.read_unlocked()?;
        let mut due = Vec::new();
        let mut occurrences = Vec::new();
        for job in &mut file.jobs {
            if job.status != ScheduleJobStatus::Active {
                continue;
            }
            let Some(scheduled_at) = job.next_run_at else {
                continue;
            };
            if scheduled_at > now {
                continue;
            }
            job.last_run_at = Some(now);
            job.run_count = job.run_count.saturating_add(1);
            job.next_run_at = next_run_from_interval(job.interval_ms, now);
            job.updated_at = now;
            let claimed = job.clone();
            if matches!(mode, DueClaimMode::WithOutbox) {
                occurrences.push(ScheduleOccurrence::new(claimed.clone(), scheduled_at));
            }
            due.push(claimed);
        }
        if !due.is_empty() {
            if matches!(mode, DueClaimMode::WithOutbox) {
                self.persist_occurrences_unlocked(&occurrences)?;
            }
            self.write_unlocked(&file)?;
        }
        Ok(due)
    }

    fn read_unlocked(&self) -> Result<SchedulesFile> {
        match fs::read_to_string(&self.path) {
            Ok(raw) if raw.trim().is_empty() => Ok(SchedulesFile::default()),
            Ok(raw) => serde_json::from_str(&raw)
                .with_context(|| format!("parse schedules file {}", self.path.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(SchedulesFile::default()),
            Err(error) => {
                Err(error).with_context(|| format!("read schedules file {}", self.path.display()))
            }
        }
    }

    fn write_unlocked(&self, file: &SchedulesFile) -> Result<()> {
        outbox::write_atomic_json(&self.path, file, "schedule file")
    }
}

fn new_job(
    params: SessionScheduleUpsertParams,
    text: String,
    interval_ms: Option<u64>,
    next_run_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> ScheduleJob {
    let (prompt, instruction) = match params.kind {
        ScheduleJobKind::Cron => (Some(text).filter(|s| !s.is_empty()), None),
        ScheduleJobKind::Heartbeat => (None, Some(text).filter(|s| !s.is_empty())),
    };
    ScheduleJob {
        job_id: JobId::new(),
        kind: params.kind,
        status: ScheduleJobStatus::Active,
        session_id: params.session_id,
        cwd: params.cwd,
        schedule: params.schedule,
        interval_ms,
        prompt,
        instruction,
        delivery_mode: params.delivery_mode,
        label: params.label,
        created_at: now,
        updated_at: now,
        next_run_at,
        last_run_at: None,
        run_count: 0,
    }
}

/// Parses `every 5m` / `5m` / `every 30s` / `1h` style intervals.
pub(crate) fn parse_interval_ms(schedule: &str) -> Option<u64> {
    let trimmed = schedule.trim().to_ascii_lowercase();
    let rest = trimmed
        .strip_prefix("every ")
        .unwrap_or(trimmed.as_str())
        .trim();
    if rest.is_empty() {
        return None;
    }
    let (num, unit) = rest.split_at(
        rest.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len()),
    );
    let amount: u64 = num.parse().ok()?;
    let unit = unit.trim();
    let multiplier = match unit {
        "s" | "sec" | "secs" | "second" | "seconds" => 1_000,
        "m" | "min" | "mins" | "minute" | "minutes" => 60_000,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000,
        _ => return None,
    };
    amount.checked_mul(multiplier)
}

fn next_run_from_interval(interval_ms: Option<u64>, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let interval_ms = interval_ms.filter(|ms| *ms > 0)?;
    let seconds = i64::try_from(interval_ms / 1_000).ok()?;
    let remaining_milliseconds = i64::try_from(interval_ms % 1_000).ok()?;
    let duration = Duration::try_seconds(seconds)?
        .checked_add(&Duration::try_milliseconds(remaining_milliseconds)?)?;
    from.checked_add_signed(duration)
}

#[cfg(test)]
mod tests {
    use devo_protocol::native::rpc_schedule::ScheduleDeliveryMode;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::*;

    /// Trace: L2-DES-APP-012
    /// Verifies: schedule upsert/list/delete round-trips through schedules.json.
    #[test]
    fn upsert_list_delete_round_trip() {
        let dir = tempdir().expect("tempdir");
        let store = ScheduleStore::new(dir.path());
        let session_id = SessionId::from_string("ses_sched_test".into());
        let upserted = store
            .upsert(SessionScheduleUpsertParams {
                kind: ScheduleJobKind::Cron,
                session_id,
                job_id: None,
                cwd: Some(dir.path().to_path_buf()),
                schedule: Some("every 5m".into()),
                interval_ms: None,
                prompt: Some("ping".into()),
                instruction: None,
                delivery_mode: ScheduleDeliveryMode::Steer,
                label: Some("cron".into()),
            })
            .expect("upsert");
        assert_eq!(upserted.interval_ms, Some(300_000));
        assert!(dir.path().join("schedules.json").is_file());

        let listed = store.list(Some(&session_id), None).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].job_id, upserted.job_id);

        let deleted = store.delete(&upserted.job_id).expect("delete");
        assert_eq!(deleted.status, ScheduleJobStatus::Stopped);
        assert!(
            store
                .list(Some(&session_id), None)
                .expect("list")
                .is_empty()
        );
    }

    #[test]
    fn parse_every_interval() {
        assert_eq!(parse_interval_ms("every 5m"), Some(300_000));
        assert_eq!(parse_interval_ms("5m"), Some(300_000));
        assert_eq!(parse_interval_ms("every 30s"), Some(30_000));
        assert_eq!(parse_interval_ms("every 1h"), Some(3_600_000));
        assert_eq!(parse_interval_ms("every 18446744073709551615h"), None);
        assert_eq!(parse_interval_ms("0 */5 * * * *"), None);
    }

    /// Regression: an unparsable or zero schedule used to store an Active
    /// job with `next_run_at: None` that silently never fired.
    #[test]
    fn upsert_rejects_schedules_that_can_never_fire() {
        let dir = tempdir().expect("tempdir");
        let store = ScheduleStore::new(dir.path());
        let session_id = SessionId::from_string("ses_sched_reject".into());

        for (schedule, interval_ms) in [
            (Some("0 9 * * *".to_string()), None),
            (Some("daily at 9am".to_string()), None),
            (Some(String::new()), None),
            (None, None),
            (Some("every 5m".to_string()), Some(0)),
            (Some("every 18446744073709551615h".to_string()), None),
            (Some("every 1s".to_string()), Some(u64::MAX)),
        ] {
            let error = store
                .upsert(SessionScheduleUpsertParams {
                    kind: ScheduleJobKind::Cron,
                    session_id,
                    job_id: None,
                    cwd: None,
                    schedule,
                    interval_ms,
                    prompt: Some("ping".into()),
                    instruction: None,
                    delivery_mode: ScheduleDeliveryMode::Steer,
                    label: None,
                })
                .expect_err("unparsable schedule must be rejected at upsert");
            assert!(
                error.to_string().starts_with("invalid schedule:"),
                "unexpected error: {error}"
            );
        }
        // Nothing was persisted.
        assert!(
            store
                .list(Some(&session_id), None)
                .expect("list")
                .is_empty(),
            "rejected upserts must not store jobs"
        );
    }

    #[test]
    fn failed_claim_retries_before_a_long_interval() {
        let dir = tempdir().expect("tempdir");
        let store = ScheduleStore::new(dir.path());
        let session_id = SessionId::from_string("ses_sched_retry".into());
        let job = store
            .upsert(SessionScheduleUpsertParams {
                kind: ScheduleJobKind::Cron,
                session_id,
                job_id: None,
                cwd: None,
                schedule: Some("every 5m".into()),
                interval_ms: None,
                prompt: Some("ping".into()),
                instruction: None,
                delivery_mode: ScheduleDeliveryMode::FollowUp,
                label: None,
            })
            .expect("upsert schedule");
        let claim_at = job.next_run_at.expect("first run time") + Duration::seconds(1);
        let claimed = store
            .claim_due(claim_at)
            .expect("claim due job")
            .pop()
            .expect("one due job");
        let retry_at = claim_at + Duration::seconds(15);

        store
            .retry_claimed(&claimed, retry_at)
            .expect("persist retry time");

        let mut expected = claimed;
        expected.next_run_at = Some(retry_at);
        let listed = store.list(Some(&session_id), None).expect("list jobs");
        expected.updated_at = listed[0].updated_at;
        assert_eq!(listed, vec![expected]);
    }

    #[test]
    fn resume_rejects_intervals_outside_supported_date_range() {
        let dir = tempdir().expect("tempdir");
        let store = ScheduleStore::new(dir.path());
        let session_id = SessionId::from_string("ses_sched_resume_overflow".into());
        let job = store
            .upsert(SessionScheduleUpsertParams {
                kind: ScheduleJobKind::Cron,
                session_id,
                job_id: None,
                cwd: None,
                schedule: Some("every 5m".into()),
                interval_ms: None,
                prompt: Some("ping".into()),
                instruction: None,
                delivery_mode: ScheduleDeliveryMode::Steer,
                label: None,
            })
            .expect("upsert valid schedule");
        let paused = store
            .update(&job.job_id, ScheduleUpdateAction::Pause)
            .expect("pause schedule");
        let mut schedules = store.read_unlocked().expect("read schedules");
        schedules.jobs[0].interval_ms = Some(u64::MAX);
        store
            .write_unlocked(&schedules)
            .expect("write invalid interval");

        let error = store
            .update(&job.job_id, ScheduleUpdateAction::Resume)
            .expect_err("out-of-range stored interval must not reactivate");
        assert!(error.to_string().starts_with("invalid schedule:"));

        let mut expected = paused;
        expected.interval_ms = Some(u64::MAX);
        assert_eq!(
            store.list(Some(&session_id), None).expect("list jobs"),
            vec![expected]
        );
    }

    #[test]
    fn labeled_heartbeats_coexist() {
        let dir = tempdir().expect("tempdir");
        let store = ScheduleStore::new(dir.path());
        let session_id = SessionId::from_string("ses_hb_multi".into());
        let first = store
            .upsert(SessionScheduleUpsertParams {
                kind: ScheduleJobKind::Heartbeat,
                session_id,
                job_id: None,
                cwd: None,
                schedule: Some("every 5m".into()),
                interval_ms: None,
                prompt: None,
                instruction: Some("check a".into()),
                delivery_mode: ScheduleDeliveryMode::Steer,
                label: Some("a".into()),
            })
            .expect("upsert a");
        let second = store
            .upsert(SessionScheduleUpsertParams {
                kind: ScheduleJobKind::Heartbeat,
                session_id,
                job_id: None,
                cwd: None,
                schedule: Some("every 10m".into()),
                interval_ms: None,
                prompt: None,
                instruction: Some("check b".into()),
                delivery_mode: ScheduleDeliveryMode::FollowUp,
                label: Some("b".into()),
            })
            .expect("upsert b");
        let listed = store.list(Some(&session_id), None).expect("list");
        assert_eq!(listed.len(), 2);
        assert_ne!(first.job_id, second.job_id);
    }

    #[test]
    fn stopped_schedule_cannot_be_paused_or_resumed() {
        let dir = tempdir().expect("tempdir");
        let store = ScheduleStore::new(dir.path());
        let session_id = SessionId::from_string("ses_sched_stopped_terminal".into());
        let job = store
            .upsert(SessionScheduleUpsertParams {
                kind: ScheduleJobKind::Cron,
                session_id,
                job_id: None,
                cwd: None,
                schedule: Some("every 5m".into()),
                interval_ms: None,
                prompt: Some("ping".into()),
                instruction: None,
                delivery_mode: ScheduleDeliveryMode::FollowUp,
                label: None,
            })
            .expect("upsert schedule");
        let stopped = store
            .update(&job.job_id, ScheduleUpdateAction::Stop)
            .expect("stop schedule");
        assert_eq!(stopped.status, ScheduleJobStatus::Stopped);

        for action in [ScheduleUpdateAction::Pause, ScheduleUpdateAction::Resume] {
            let error = store
                .update(&job.job_id, action)
                .expect_err("stopped schedules must be terminal");
            assert!(
                error
                    .to_string()
                    .starts_with("stopped schedule is terminal")
            );
        }

        let reconfigured = store
            .upsert(SessionScheduleUpsertParams {
                kind: ScheduleJobKind::Cron,
                session_id,
                job_id: Some(job.job_id),
                cwd: None,
                schedule: Some("every 10m".into()),
                interval_ms: None,
                prompt: Some("updated ping".into()),
                instruction: None,
                delivery_mode: ScheduleDeliveryMode::FollowUp,
                label: None,
            })
            .expect("upsert stopped schedule");
        assert_eq!(reconfigured.status, ScheduleJobStatus::Stopped);
        assert!(reconfigured.next_run_at.is_none());

        assert_eq!(
            store
                .list_filtered(Some(&session_id), None, true)
                .expect("list including stopped"),
            vec![reconfigured]
        );
    }
}
