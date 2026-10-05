//! Kernel `host_request("rlm_heartbeat.*")` → durable [`ScheduleStore`] heartbeats.

use serde_json::{Value, json};

use super::ServerRuntime;
use crate::schedule_store::parse_interval_ms;
use devo_protocol::native::ids::{JobId, SessionId};
use devo_protocol::native::rpc_schedule::{
    ScheduleDeliveryMode, ScheduleJob, ScheduleJobKind, ScheduleJobStatus, ScheduleUpdateAction,
    SessionScheduleUpsertParams,
};

const DEFAULT_INTERVAL: &str = "5m";

impl ServerRuntime {
    pub(crate) fn host_rlm_heartbeat_list(&self, session_id: &str, params: &Value) -> Value {
        let Some(sid) = parse_sid(session_id) else {
            return error_reply("invalid session id");
        };
        let include_inactive = params
            .get("include_inactive")
            .or_else(|| params.get("includeInactive"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        match self.schedule_store.list_filtered(
            Some(&sid),
            None,
            /*include_stopped*/ include_inactive,
        ) {
            Ok(jobs) => {
                let heartbeats: Vec<Value> = jobs
                    .into_iter()
                    .filter(|job| job.kind == ScheduleJobKind::Heartbeat)
                    .filter(|job| {
                        include_inactive
                            || matches!(
                                job.status,
                                ScheduleJobStatus::Active | ScheduleJobStatus::Paused
                            )
                    })
                    .map(job_to_heartbeat_json)
                    .collect();
                ok_result(json!({ "heartbeats": heartbeats }))
            }
            Err(err) => error_reply(err.to_string()),
        }
    }

    pub(crate) async fn host_rlm_heartbeat_create(
        &self,
        session_id: &str,
        params: &Value,
    ) -> Value {
        let Some(sid) = parse_sid(session_id) else {
            return error_reply("invalid session id");
        };
        let instruction = params
            .get("instruction")
            .or_else(|| params.get("prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if instruction.is_empty() {
            return error_reply("rlm_heartbeat.create requires instruction");
        }
        let interval = params
            .get("interval")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_INTERVAL)
            .to_string();
        let Some(interval_ms) = parse_interval_ms(&interval) else {
            return error_reply(format!(
                "invalid interval '{interval}' (expected e.g. 5m, every 30s, 1h)"
            ));
        };
        let schedule = normalize_every_schedule(&interval);
        let label = params
            .get("label")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let delivery_mode = parse_delivery_mode(params.get("delivery_mode"));
        let upsert = SessionScheduleUpsertParams {
            kind: ScheduleJobKind::Heartbeat,
            session_id: sid,
            job_id: None,
            cwd: None,
            schedule: Some(schedule),
            interval_ms: Some(interval_ms),
            prompt: None,
            instruction: Some(instruction),
            delivery_mode,
            label,
        };
        let _dispatch_guard = self.schedule_store.lock_dispatch().await;
        match self.schedule_store.upsert(upsert) {
            Ok(job) => ok_result(job_to_heartbeat_json(job)),
            Err(err) => error_reply(err.to_string()),
        }
    }

    pub(crate) async fn host_rlm_heartbeat_update(
        &self,
        session_id: &str,
        params: &Value,
    ) -> Value {
        let Some(sid) = parse_sid(session_id) else {
            return error_reply("invalid session id");
        };
        let Some(job_id) = params
            .get("id")
            .or_else(|| params.get("job_id"))
            .or_else(|| params.get("jobId"))
            .and_then(|v| v.as_str())
            .map(|s| JobId::from_string(s.to_owned()))
        else {
            return error_reply("rlm_heartbeat.update requires id");
        };

        // Keep ownership checks and subsequent updates atomic with dispatch.
        let _dispatch_guard = self.schedule_store.lock_dispatch().await;
        if let Some(status) = params.get("status").and_then(|v| v.as_str()) {
            let action = match status {
                "pause" => ScheduleUpdateAction::Pause,
                "resume" => ScheduleUpdateAction::Resume,
                other => {
                    return error_reply(format!(
                        "rlm_heartbeat.update status must be pause|resume, got '{other}'"
                    ));
                }
            };
            if let Err(err) = self.require_session_heartbeat(&sid, &job_id) {
                return error_reply(err);
            }
            return match self.schedule_store.update(&job_id, action) {
                Ok(job) => ok_result(job_to_heartbeat_json(job)),
                Err(err) => error_reply(err.to_string()),
            };
        }

        let existing = match self.require_session_heartbeat(&sid, &job_id) {
            Ok(job) => job,
            Err(err) => return error_reply(err),
        };

        let instruction = params
            .get("instruction")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| existing.instruction.clone());
        let interval_raw = params
            .get("interval")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let (schedule, interval_ms) = if let Some(ref interval) = interval_raw {
            let Some(ms) = parse_interval_ms(interval) else {
                return error_reply(format!(
                    "invalid interval '{interval}' (expected e.g. 5m, every 30s, 1h)"
                ));
            };
            (Some(normalize_every_schedule(interval)), Some(ms))
        } else {
            (existing.schedule.clone(), existing.interval_ms)
        };
        let label = if params.get("label").is_some() {
            params
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        } else {
            existing.label.clone()
        };
        let delivery_mode = if params.get("delivery_mode").is_some() {
            parse_delivery_mode(params.get("delivery_mode"))
        } else {
            existing.delivery_mode
        };

        let upsert = SessionScheduleUpsertParams {
            kind: ScheduleJobKind::Heartbeat,
            session_id: sid,
            job_id: Some(job_id),
            cwd: existing.cwd.clone(),
            schedule,
            interval_ms,
            prompt: None,
            instruction,
            delivery_mode,
            label,
        };
        match self.schedule_store.upsert(upsert) {
            Ok(job) => ok_result(job_to_heartbeat_json(job)),
            Err(err) => error_reply(err.to_string()),
        }
    }

    pub(crate) async fn host_rlm_heartbeat_delete(
        &self,
        session_id: &str,
        params: &Value,
    ) -> Value {
        let Some(sid) = parse_sid(session_id) else {
            return error_reply("invalid session id");
        };
        let Some(job_id) = params
            .get("id")
            .or_else(|| params.get("job_id"))
            .or_else(|| params.get("jobId"))
            .and_then(|v| v.as_str())
            .map(|s| JobId::from_string(s.to_owned()))
        else {
            return error_reply("rlm_heartbeat.delete requires id");
        };
        let _dispatch_guard = self.schedule_store.lock_dispatch().await;
        if let Err(err) = self.require_session_heartbeat(&sid, &job_id) {
            return error_reply(err);
        }
        match self.schedule_store.delete(&job_id) {
            Ok(job) => ok_result(job_to_heartbeat_json(job)),
            Err(err) => error_reply(err.to_string()),
        }
    }

    fn require_session_heartbeat(
        &self,
        session_id: &SessionId,
        job_id: &JobId,
    ) -> Result<ScheduleJob, String> {
        let jobs = self
            .schedule_store
            .list(Some(session_id), None)
            .map_err(|e| e.to_string())?;
        jobs.into_iter()
            .find(|job| &job.job_id == job_id && job.kind == ScheduleJobKind::Heartbeat)
            .ok_or_else(|| format!("heartbeat not found: {}", job_id.as_str()))
    }
}

fn parse_sid(session_id: &str) -> Option<SessionId> {
    session_id.parse().ok()
}

fn parse_delivery_mode(value: Option<&Value>) -> ScheduleDeliveryMode {
    match value.and_then(|v| v.as_str()).unwrap_or("steer") {
        "follow_up" | "follow-up" | "followUp" => ScheduleDeliveryMode::FollowUp,
        _ => ScheduleDeliveryMode::Steer,
    }
}

fn normalize_every_schedule(interval: &str) -> String {
    let trimmed = interval.trim();
    if trimmed.to_ascii_lowercase().starts_with("every ") {
        trimmed.to_string()
    } else {
        format!("every {trimmed}")
    }
}

fn job_to_heartbeat_json(job: ScheduleJob) -> Value {
    let status = match job.status {
        ScheduleJobStatus::Active => "active",
        ScheduleJobStatus::Paused => "paused",
        ScheduleJobStatus::Stopped => "stopped",
    };
    let delivery_mode = match job.delivery_mode {
        ScheduleDeliveryMode::Steer => "steer",
        ScheduleDeliveryMode::FollowUp => "follow_up",
    };
    let interval = job
        .schedule
        .as_deref()
        .and_then(|s| s.strip_prefix("every ").map(str::trim))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            job.interval_ms.map(|ms| {
                if ms % 3_600_000 == 0 {
                    format!("{}h", ms / 3_600_000)
                } else if ms % 60_000 == 0 {
                    format!("{}m", ms / 60_000)
                } else {
                    format!("{}s", ms / 1_000)
                }
            })
        });
    json!({
        "id": job.job_id.as_str(),
        "jobId": job.job_id.as_str(),
        "instruction": job.instruction,
        "interval": interval,
        "intervalMs": job.interval_ms,
        "label": job.label,
        "status": status,
        "deliveryMode": delivery_mode,
        "nextRunAt": job.next_run_at,
        "lastRunAt": job.last_run_at,
        "runCount": job.run_count,
    })
}

fn ok_result(result: Value) -> Value {
    json!({ "status": "ok", "result": result })
}

fn error_reply(msg: impl Into<String>) -> Value {
    json!({ "status": "error", "error": msg.into() })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::schedule_store::parse_interval_ms;

    #[test]
    fn bare_interval_parses_like_every_prefix() {
        assert_eq!(parse_interval_ms("5m"), Some(300_000));
        assert_eq!(parse_interval_ms("every 5m"), Some(300_000));
        assert_eq!(parse_interval_ms("30s"), Some(30_000));
    }
}
