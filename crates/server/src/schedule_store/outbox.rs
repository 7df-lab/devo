use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use devo_protocol::native::rpc_schedule::ScheduleJob;

use super::{DueClaimMode, ScheduleStore};

const OUTBOX_FILE_NAME: &str = "schedule-outbox.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScheduleOccurrence {
    pub(crate) occurrence_id: String,
    pub(crate) scheduled_at: DateTime<Utc>,
    pub(crate) job: ScheduleJob,
    attempt_count: u32,
    retry_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) delivery_attempted_at: Option<DateTime<Utc>>,
}

impl ScheduleOccurrence {
    pub(super) fn new(job: ScheduleJob, scheduled_at: DateTime<Utc>) -> Self {
        Self {
            occurrence_id: schedule_occurrence_id(&job, scheduled_at),
            scheduled_at,
            job,
            attempt_count: 0,
            retry_at: None,
            delivery_attempted_at: None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ScheduleOutboxFile {
    #[serde(default)]
    occurrences: Vec<ScheduleOccurrence>,
}

impl ScheduleStore {
    pub(crate) fn claim_due_with_outbox(&self, now: DateTime<Utc>) -> Result<Vec<ScheduleJob>> {
        self.claim_due_inner(now, DueClaimMode::WithOutbox)
    }

    pub(crate) fn pending_occurrences(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ScheduleOccurrence>> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let mut occurrences = self
            .read_outbox_unlocked()?
            .occurrences
            .into_iter()
            .filter(|occurrence| occurrence.retry_at.is_none_or(|retry_at| retry_at <= now))
            .collect::<Vec<_>>();
        occurrences.sort_by(|left, right| {
            left.scheduled_at
                .cmp(&right.scheduled_at)
                .then_with(|| left.occurrence_id.cmp(&right.occurrence_id))
        });
        Ok(occurrences)
    }

    pub(crate) fn acknowledge_occurrence(&self, occurrence_id: &str) -> Result<bool> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let mut outbox = self.read_outbox_unlocked()?;
        let old_len = outbox.occurrences.len();
        outbox
            .occurrences
            .retain(|occurrence| occurrence.occurrence_id != occurrence_id);
        if outbox.occurrences.len() == old_len {
            return Ok(false);
        }
        self.write_outbox_unlocked(&outbox)?;
        self.clear_occurrence_inflight(occurrence_id);
        Ok(true)
    }

    pub(crate) fn mark_occurrence_delivery_attempted(
        &self,
        occurrence_id: &str,
        attempted_at: DateTime<Utc>,
    ) -> Result<bool> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let mut outbox = self.read_outbox_unlocked()?;
        let Some(occurrence) = outbox
            .occurrences
            .iter_mut()
            .find(|occurrence| occurrence.occurrence_id == occurrence_id)
        else {
            return Ok(false);
        };
        occurrence.delivery_attempted_at = Some(attempted_at);
        occurrence.retry_at = None;
        self.write_outbox_unlocked(&outbox)?;
        Ok(true)
    }

    pub(crate) fn retry_occurrence(
        &self,
        occurrence_id: &str,
        retry_at: DateTime<Utc>,
    ) -> Result<bool> {
        let _guard = self.lock.lock().expect("schedule store mutex poisoned");
        let mut outbox = self.read_outbox_unlocked()?;
        let Some(occurrence) = outbox
            .occurrences
            .iter_mut()
            .find(|occurrence| occurrence.occurrence_id == occurrence_id)
        else {
            return Ok(false);
        };
        occurrence.attempt_count = occurrence.attempt_count.saturating_add(1);
        occurrence.retry_at = Some(retry_at);
        self.write_outbox_unlocked(&outbox)?;
        self.clear_occurrence_inflight(occurrence_id);
        Ok(true)
    }

    pub(crate) fn mark_occurrence_inflight(&self, occurrence_id: &str, admitted_at: DateTime<Utc>) {
        self.inflight_occurrences
            .lock()
            .expect("schedule in-flight mutex poisoned")
            .entry(occurrence_id.to_string())
            .or_insert(admitted_at);
    }

    pub(crate) fn try_reserve_occurrence(
        &self,
        occurrence_id: &str,
        reserved_at: DateTime<Utc>,
    ) -> bool {
        let mut inflight = self
            .inflight_occurrences
            .lock()
            .expect("schedule in-flight mutex poisoned");
        if inflight.contains_key(occurrence_id) {
            return false;
        }
        inflight.insert(occurrence_id.to_string(), reserved_at);
        true
    }

    pub(crate) fn occurrence_inflight_since(&self, occurrence_id: &str) -> Option<DateTime<Utc>> {
        self.inflight_occurrences
            .lock()
            .expect("schedule in-flight mutex poisoned")
            .get(occurrence_id)
            .copied()
    }

    pub(crate) fn clear_occurrence_inflight(&self, occurrence_id: &str) {
        self.inflight_occurrences
            .lock()
            .expect("schedule in-flight mutex poisoned")
            .remove(occurrence_id);
    }

    pub(super) fn persist_occurrences_unlocked(
        &self,
        occurrences: &[ScheduleOccurrence],
    ) -> Result<()> {
        let mut outbox = self.read_outbox_unlocked()?;
        for occurrence in occurrences {
            if !outbox
                .occurrences
                .iter()
                .any(|existing| existing.occurrence_id == occurrence.occurrence_id)
            {
                outbox.occurrences.push(occurrence.clone());
            }
        }
        self.write_outbox_unlocked(&outbox)
    }

    fn read_outbox_unlocked(&self) -> Result<ScheduleOutboxFile> {
        let path = self.path.with_file_name(OUTBOX_FILE_NAME);
        match fs::read_to_string(&path) {
            Ok(raw) if raw.trim().is_empty() => Ok(ScheduleOutboxFile::default()),
            Ok(raw) => serde_json::from_str(&raw)
                .with_context(|| format!("parse schedule outbox {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(ScheduleOutboxFile::default())
            }
            Err(error) => {
                Err(error).with_context(|| format!("read schedule outbox {}", path.display()))
            }
        }
    }

    fn write_outbox_unlocked(&self, outbox: &ScheduleOutboxFile) -> Result<()> {
        write_atomic_json(
            &self.path.with_file_name(OUTBOX_FILE_NAME),
            outbox,
            "schedule outbox",
        )
    }
}

fn schedule_occurrence_id(job: &ScheduleJob, scheduled_at: DateTime<Utc>) -> String {
    format!(
        "schedule:{}:{}",
        job.job_id.as_str(),
        scheduled_at.timestamp_micros()
    )
}

pub(super) fn write_atomic_json<T: Serialize>(path: &Path, value: &T, label: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("schedule persistence path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create schedule directory {}", parent.display()))?;
    let raw = serde_json::to_vec_pretty(value).context("serialize schedule persistence file")?;
    let tmp = path.with_extension("json.tmp");
    let mut file = File::create(&tmp)
        .with_context(|| format!("create {label} temp file {}", tmp.display()))?;
    file.write_all(&raw)
        .with_context(|| format!("write {label} temp file {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {label} temp file {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "rename {label} temp file {} to {}",
            tmp.display(),
            path.display()
        )
    })?;
    sync_parent_directory(parent, label)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path, label: &str) -> Result<()> {
    File::open(parent)
        .with_context(|| format!("open {label} parent directory {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("sync {label} parent directory {}", parent.display()))
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path, _label: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use devo_protocol::native::ids::SessionId;
    use devo_protocol::native::rpc_schedule::ScheduleDeliveryMode;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    fn store_with_due_job() -> (TempDir, ScheduleStore, DateTime<Utc>) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let store = ScheduleStore::new(dir.path());
        store
            .upsert(
                devo_protocol::native::rpc_schedule::SessionScheduleUpsertParams {
                    kind: devo_protocol::native::rpc_schedule::ScheduleJobKind::Cron,
                    session_id: SessionId::from_string("ses_outbox_test".to_string()),
                    job_id: None,
                    cwd: Some(dir.path().to_path_buf()),
                    schedule: Some("every 5m".to_string()),
                    interval_ms: None,
                    prompt: Some("stable scheduled prompt".to_string()),
                    instruction: None,
                    delivery_mode: ScheduleDeliveryMode::FollowUp,
                    label: None,
                },
            )
            .expect("create scheduled job");
        let scheduled_at = Utc::now() - Duration::seconds(1);
        let mut schedules = store.read_unlocked().expect("read schedule file");
        schedules.jobs[0].next_run_at = Some(scheduled_at);
        store.write_unlocked(&schedules).expect("make schedule due");
        (dir, store, scheduled_at)
    }

    #[test]
    fn outbox_is_separate_from_legacy_schedule_file_and_survives_reopen() {
        let (dir, store, scheduled_at) = store_with_due_job();
        let due = store
            .claim_due_with_outbox(Utc::now())
            .expect("claim due job");
        assert_eq!(due.len(), 1);

        let schedule_json: serde_json::Value = serde_json::from_slice(
            &fs::read(dir.path().join("schedules.json")).expect("read legacy schedule file"),
        )
        .expect("parse legacy schedule file");
        assert!(schedule_json.get("jobs").is_some());
        assert!(schedule_json.get("occurrences").is_none());

        let sidecar = dir.path().join(OUTBOX_FILE_NAME);
        assert!(sidecar.is_file());
        let expected_id = schedule_occurrence_id(&due[0], scheduled_at);
        drop(store);
        let reopened = ScheduleStore::new(dir.path());
        let occurrences = reopened
            .pending_occurrences(Utc::now())
            .expect("read reopened outbox");
        assert_eq!(occurrences.len(), 1);
        assert_eq!(occurrences[0].occurrence_id, expected_id);
        assert_eq!(occurrences[0].scheduled_at, scheduled_at);
        assert_eq!(
            occurrences[0].job.prompt.as_deref(),
            Some("stable scheduled prompt")
        );
    }

    #[test]
    fn crash_after_outbox_write_reuses_the_same_due_occurrence() {
        let (dir, store, scheduled_at) = store_with_due_job();
        let schedules = store.read_unlocked().expect("read schedule file");
        let job = schedules.jobs[0].clone();
        let occurrence = ScheduleOccurrence::new(job.clone(), scheduled_at);
        store
            .persist_occurrences_unlocked(std::slice::from_ref(&occurrence))
            .expect("persist outbox before advancing schedule");
        drop(store);

        let reopened = ScheduleStore::new(dir.path());
        let reclaimed = reopened
            .claim_due_with_outbox(Utc::now())
            .expect("reconcile and reclaim due job");
        assert_eq!(reclaimed.len(), 1);
        let occurrences = reopened
            .pending_occurrences(Utc::now())
            .expect("read reconciled outbox");
        assert_eq!(occurrences.len(), 1);
        assert_eq!(occurrences[0], occurrence);
    }

    #[test]
    fn outbox_retry_and_acknowledgement_are_durable() {
        let (dir, store, _) = store_with_due_job();
        store
            .claim_due_with_outbox(Utc::now())
            .expect("claim due job");
        let occurrence_id = store
            .pending_occurrences(Utc::now())
            .expect("read claimed occurrence")
            .into_iter()
            .next()
            .expect("claimed occurrence")
            .occurrence_id;
        let attempted_at = Utc::now();
        assert!(
            store
                .mark_occurrence_delivery_attempted(&occurrence_id, attempted_at)
                .expect("persist delivery attempt")
        );
        let retry_at = Utc::now() + Duration::seconds(30);
        assert!(
            store
                .retry_occurrence(&occurrence_id, retry_at)
                .expect("persist retry")
        );
        assert!(
            store
                .pending_occurrences(Utc::now())
                .expect("before retry time")
                .is_empty()
        );
        drop(store);

        let reopened = ScheduleStore::new(dir.path());
        let ready = reopened
            .pending_occurrences(retry_at)
            .expect("after retry time");
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].delivery_attempted_at, Some(attempted_at));
        assert!(
            reopened
                .acknowledge_occurrence(&occurrence_id)
                .expect("acknowledge")
        );
        assert!(
            reopened
                .pending_occurrences(Utc::now())
                .expect("after acknowledgement")
                .is_empty()
        );
    }
}
