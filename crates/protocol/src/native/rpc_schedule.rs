//! Params/result types for `session/schedule/*` and `session/export|import`.
//!
//! Schedule jobs are server-persisted (`~/.devo/schedules.json`) and wake
//! outside the session actor mailbox (L2-DES-SERVER-002).

use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ids::JobId;
use super::ids::SessionId;

// ── Schedule domain types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleJobKind {
    Cron,
    Heartbeat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleDeliveryMode {
    #[default]
    Steer,
    FollowUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleJobStatus {
    Active,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleUpdateAction {
    Pause,
    Resume,
    Stop,
}

/// Durable scheduled job (cron or heartbeat).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleJob {
    pub job_id: JobId,
    pub kind: ScheduleJobKind,
    pub status: ScheduleJobStatus,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Cron / "every 5m"-style expression when [`Self::kind`] is Cron or when
    /// the heartbeat was created from a schedule string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    /// Prompt text for cron jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Instruction text for heartbeat jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    pub delivery_mode: ScheduleDeliveryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    pub run_count: u64,
}

// ── session/schedule/list ──

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionScheduleListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionScheduleListResult {
    pub jobs: Vec<ScheduleJob>,
}

// ── session/schedule/upsert ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionScheduleUpsertParams {
    pub kind: ScheduleJobKind,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<JobId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    #[serde(default)]
    pub delivery_mode: ScheduleDeliveryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionScheduleUpsertResult {
    pub job: ScheduleJob,
}

// ── session/schedule/update ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionScheduleUpdateParams {
    pub job_id: JobId,
    pub action: ScheduleUpdateAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionScheduleUpdateResult {
    pub job: ScheduleJob,
}

// ── session/schedule/delete ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionScheduleDeleteParams {
    pub job_id: JobId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionScheduleDeleteResult {}

// ── session/heartbeat/command ──
//
// User `/heartbeat` slash surface: parse + apply on the server so schedule
// capability stays backend-owned (persist / wake / one-per-session unlabeled).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum HeartbeatCommandAction {
    Status,
    Set,
    Pause,
    Resume,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeartbeatCommandParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Text after `/heartbeat` (empty or `status` → status).
    #[serde(default)]
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeartbeatCommandResult {
    pub action: HeartbeatCommandAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<ScheduleJob>,
}

// ── session/export ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum SessionExportFormat {
    Jsonl,
    Html,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionExportParams {
    pub session_id: SessionId,
    pub format: SessionExportFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionExportResult {
    pub path: PathBuf,
}

// ── session/import ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum SessionImportFormat {
    #[default]
    Jsonl,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionImportParams {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<SessionImportFormat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionImportResult {
    pub session_id: SessionId,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn schedule_upsert_params_round_trip_camel_case() {
        let params = SessionScheduleUpsertParams {
            kind: ScheduleJobKind::Heartbeat,
            session_id: SessionId::from_string("ses_test".into()),
            job_id: None,
            cwd: None,
            schedule: Some("every 5m".into()),
            interval_ms: Some(300_000),
            prompt: None,
            instruction: Some("check status".into()),
            delivery_mode: ScheduleDeliveryMode::FollowUp,
            label: Some("hb".into()),
        };
        let json = serde_json::to_value(&params).expect("serialize");
        assert_eq!(json["kind"], "heartbeat");
        assert_eq!(json["sessionId"], "ses_test");
        assert_eq!(json["intervalMs"], 300_000);
        assert_eq!(json["deliveryMode"], "followUp");
        let back: SessionScheduleUpsertParams = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, params);
    }
}
