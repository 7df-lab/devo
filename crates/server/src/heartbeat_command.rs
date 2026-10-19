//! User `/heartbeat` command parsing and apply (server-owned).
//!
//! Clients send the raw args after `/heartbeat`; this module interprets them
//! and mutates [`crate::schedule_store::ScheduleStore`]. Interval parsing and
//! wake remain in the schedule store / wake loop.

use anyhow::{Result, bail};
use devo_protocol::native::ids::SessionId;
use devo_protocol::native::rpc_schedule::{
    HeartbeatCommandAction, ScheduleDeliveryMode, ScheduleJob, ScheduleJobKind, ScheduleJobStatus,
    ScheduleUpdateAction, SessionHeartbeatCommandParams, SessionHeartbeatCommandResult,
    SessionScheduleUpsertParams,
};

use crate::schedule_store::{ScheduleStore, parse_interval_ms};

pub(crate) const DEFAULT_HEARTBEAT_SCHEDULE: &str = "every 5m";

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedHeartbeatCommand {
    Status,
    Pause,
    Resume,
    Clear,
    Set {
        schedule: String,
        instruction: String,
        delivery_mode: ScheduleDeliveryMode,
    },
}

/// Parse + apply a user `/heartbeat` command against the schedule store.
pub(crate) fn run_heartbeat_command(
    store: &ScheduleStore,
    params: SessionHeartbeatCommandParams,
) -> Result<SessionHeartbeatCommandResult> {
    let parsed = parse_heartbeat_command(&params.args)?;
    match parsed {
        ParsedHeartbeatCommand::Status => Ok(SessionHeartbeatCommandResult {
            action: HeartbeatCommandAction::Status,
            job: find_user_heartbeat(store, &params.session_id)?,
        }),
        ParsedHeartbeatCommand::Pause => mutate_user_heartbeat(
            store,
            &params.session_id,
            HeartbeatCommandAction::Pause,
            ScheduleUpdateAction::Pause,
        ),
        ParsedHeartbeatCommand::Resume => mutate_user_heartbeat(
            store,
            &params.session_id,
            HeartbeatCommandAction::Resume,
            ScheduleUpdateAction::Resume,
        ),
        ParsedHeartbeatCommand::Clear => {
            let Some(current) = find_user_heartbeat(store, &params.session_id)? else {
                return Ok(SessionHeartbeatCommandResult {
                    action: HeartbeatCommandAction::Clear,
                    job: None,
                });
            };
            let job = store.delete(&current.job_id)?;
            Ok(SessionHeartbeatCommandResult {
                action: HeartbeatCommandAction::Clear,
                job: Some(job),
            })
        }
        ParsedHeartbeatCommand::Set {
            schedule,
            instruction,
            delivery_mode,
        } => {
            if parse_interval_ms(&schedule).is_none() {
                bail!("invalid heartbeat schedule: {schedule}");
            }
            let job = store.upsert(SessionScheduleUpsertParams {
                kind: ScheduleJobKind::Heartbeat,
                session_id: params.session_id,
                job_id: None,
                cwd: params.cwd,
                schedule: Some(schedule),
                interval_ms: None,
                prompt: None,
                instruction: Some(instruction),
                delivery_mode,
                label: None,
            })?;
            Ok(SessionHeartbeatCommandResult {
                action: HeartbeatCommandAction::Set,
                job: Some(job),
            })
        }
    }
}

fn mutate_user_heartbeat(
    store: &ScheduleStore,
    session_id: &SessionId,
    action: HeartbeatCommandAction,
    update: ScheduleUpdateAction,
) -> Result<SessionHeartbeatCommandResult> {
    let Some(current) = find_user_heartbeat(store, session_id)? else {
        return Ok(SessionHeartbeatCommandResult { action, job: None });
    };
    let job = store.update(&current.job_id, update)?;
    Ok(SessionHeartbeatCommandResult {
        action,
        job: Some(job),
    })
}

fn find_user_heartbeat(
    store: &ScheduleStore,
    session_id: &SessionId,
) -> Result<Option<ScheduleJob>> {
    let jobs = store.list(Some(session_id), /*cwd*/ None)?;
    Ok(jobs.into_iter().rev().find(|job| {
        job.kind == ScheduleJobKind::Heartbeat
            && job.label.as_ref().is_none_or(|l| l.trim().is_empty())
            && matches!(
                job.status,
                ScheduleJobStatus::Active | ScheduleJobStatus::Paused
            )
    }))
}

fn parse_heartbeat_command(input: &str) -> Result<ParsedHeartbeatCommand> {
    let text = input
        .trim()
        .strip_prefix("/heartbeat")
        .map(str::trim)
        .unwrap_or_else(|| input.trim())
        .trim();
    if text.is_empty() || text.eq_ignore_ascii_case("status") {
        return Ok(ParsedHeartbeatCommand::Status);
    }
    if text.eq_ignore_ascii_case("pause") {
        return Ok(ParsedHeartbeatCommand::Pause);
    }
    if text.eq_ignore_ascii_case("resume") {
        return Ok(ParsedHeartbeatCommand::Resume);
    }
    if text.eq_ignore_ascii_case("clear") || text.eq_ignore_ascii_case("stop") {
        return Ok(ParsedHeartbeatCommand::Clear);
    }

    let (delivery_mode, remaining) = extract_delivery_mode(text)?;
    let remaining = remaining.trim();
    if remaining.is_empty() {
        bail!("Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>");
    }

    if let Some((schedule, instruction)) = split_leading_every(remaining) {
        if instruction.is_empty() {
            bail!("Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>");
        }
        return Ok(ParsedHeartbeatCommand::Set {
            schedule: normalize_heartbeat_schedule(&schedule),
            instruction,
            delivery_mode,
        });
    }

    if let Some(rest) = strip_prefix_ci(remaining, "--every") {
        let rest = rest
            .strip_prefix('=')
            .or_else(|| rest.strip_prefix(|c: char| c.is_whitespace()))
            .unwrap_or(rest)
            .trim_start();
        let (schedule, instruction) = split_first_interval_token(rest)?;
        if instruction.is_empty() {
            bail!("Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>");
        }
        return Ok(ParsedHeartbeatCommand::Set {
            schedule: normalize_heartbeat_schedule(&schedule),
            instruction,
            delivery_mode,
        });
    }

    // A bare leading duration is the spelling users reach for first
    // (`/heartbeat 40s check the build`); swallowing it into the instruction
    // silently changed the cadence to the 5m default.
    if let Some((schedule, instruction)) = split_leading_bare_duration(remaining) {
        if instruction.is_empty() {
            bail!("Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>");
        }
        return Ok(ParsedHeartbeatCommand::Set {
            schedule: normalize_heartbeat_schedule(&schedule),
            instruction,
            delivery_mode,
        });
    }

    Ok(ParsedHeartbeatCommand::Set {
        schedule: DEFAULT_HEARTBEAT_SCHEDULE.to_string(),
        instruction: remaining.to_string(),
        delivery_mode,
    })
}

fn normalize_heartbeat_schedule(input: &str) -> String {
    let text = input.trim();
    if text.is_empty() {
        return DEFAULT_HEARTBEAT_SCHEDULE.to_string();
    }
    if is_bare_duration(text) {
        format!("every {text}")
    } else {
        text.to_string()
    }
}

fn extract_delivery_mode(text: &str) -> Result<(ScheduleDeliveryMode, String)> {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    let mut mode = ScheduleDeliveryMode::Steer;
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        let lower = tok.to_ascii_lowercase();
        if lower == "--steer" {
            mode = ScheduleDeliveryMode::Steer;
            tokens.remove(i);
            continue;
        }
        if lower == "--follow-up" || lower == "--follow_up" {
            mode = ScheduleDeliveryMode::FollowUp;
            tokens.remove(i);
            continue;
        }
        if let Some(value) = lower.strip_prefix("--deliver=") {
            mode = parse_delivery_mode_token(value)?;
            tokens.remove(i);
            continue;
        }
        if lower == "--deliver" {
            let value = tokens.get(i + 1).copied().ok_or_else(|| {
                anyhow::anyhow!("Heartbeat delivery mode must be \"steer\" or \"follow_up\"")
            })?;
            mode = parse_delivery_mode_token(value)?;
            tokens.remove(i + 1);
            tokens.remove(i);
            continue;
        }
        i += 1;
    }
    Ok((mode, tokens.join(" ")))
}

fn parse_delivery_mode_token(token: &str) -> Result<ScheduleDeliveryMode> {
    match token.to_ascii_lowercase().replace('-', "_").as_str() {
        "steer" => Ok(ScheduleDeliveryMode::Steer),
        "follow_up" => Ok(ScheduleDeliveryMode::FollowUp),
        _ => bail!("Heartbeat delivery mode must be \"steer\" or \"follow_up\""),
    }
}

fn split_leading_every(text: &str) -> Option<(String, String)> {
    let lower = text.to_ascii_lowercase();
    let prefix_len = if lower.starts_with("every ") {
        "every ".len()
    } else if lower.starts_with("each ") {
        "each ".len()
    } else {
        return None;
    };
    let after = &text[prefix_len..];
    let (amount, rest) = after.split_once(char::is_whitespace)?;
    if amount.is_empty() || !amount.chars().next()?.is_ascii_digit() {
        return None;
    }
    // amount may be "30s" or "30" + unit already glued
    if is_bare_duration(amount) || amount.chars().all(|c| c.is_ascii_digit()) {
        // If amount is bare digits, unit is next token
        if amount.chars().all(|c| c.is_ascii_digit()) {
            let (unit, instruction) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if !is_duration_unit(unit) {
                return None;
            }
            return Some((
                format!("every {amount}{unit}"),
                instruction.trim().to_string(),
            ));
        }
        return Some((format!("every {amount}"), rest.trim().to_string()));
    }
    None
}

fn split_first_interval_token(text: &str) -> Result<(String, String)> {
    let text = text.trim();
    if text.is_empty() {
        bail!("Usage: /heartbeat [--every <interval>] [--steer|--follow-up] <instruction>");
    }
    if let Some(rest) = text.strip_prefix('"') {
        let end = rest
            .find('"')
            .ok_or_else(|| anyhow::anyhow!("unclosed quoted heartbeat interval"))?;
        return Ok((rest[..end].to_string(), rest[end + 1..].trim().to_string()));
    }
    let mut parts = text.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("").to_string();
    let rest = parts.next().unwrap_or("").trim().to_string();
    Ok((first, rest))
}

fn is_bare_duration(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let digit_end = lower
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(lower.len());
    if digit_end == 0 {
        return false;
    }
    is_duration_unit(lower[digit_end..].trim())
}

/// Splits a leading glued duration token (`40s`, `2h`) off the instruction.
///
/// Only single-token durations qualify: a bare number followed by a unit word
/// (`5 minutes remaining …`) reads as prose, so it stays part of the
/// instruction and the schedule falls back to the default.
fn split_leading_bare_duration(text: &str) -> Option<(String, String)> {
    let (first, rest) = match text.split_once(char::is_whitespace) {
        Some((first, rest)) => (first, rest.trim().to_string()),
        // The whole input is one token: a bare duration alone has no
        // instruction; surface it so the caller bails with usage.
        None => (text, String::new()),
    };
    if !is_bare_duration(first) {
        return None;
    }
    Some((first.to_string(), rest))
}

fn is_duration_unit(unit: &str) -> bool {
    matches!(
        unit.to_ascii_lowercase().as_str(),
        "s" | "sec"
            | "secs"
            | "second"
            | "seconds"
            | "m"
            | "min"
            | "mins"
            | "minute"
            | "minutes"
            | "h"
            | "hr"
            | "hrs"
            | "hour"
            | "hours"
    )
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if text.len() >= prefix.len() && text[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&text[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_status_and_lifecycle_verbs() {
        assert_eq!(
            parse_heartbeat_command("").unwrap(),
            ParsedHeartbeatCommand::Status
        );
        assert_eq!(
            parse_heartbeat_command("pause").unwrap(),
            ParsedHeartbeatCommand::Pause
        );
        assert_eq!(
            parse_heartbeat_command("clear").unwrap(),
            ParsedHeartbeatCommand::Clear
        );
    }

    #[test]
    fn parses_set_with_every_schedule() {
        assert_eq!(
            parse_heartbeat_command("every 30s QA_USER_HB").unwrap(),
            ParsedHeartbeatCommand::Set {
                schedule: "every 30s".into(),
                instruction: "QA_USER_HB".into(),
                delivery_mode: ScheduleDeliveryMode::Steer,
            }
        );
    }

    #[test]
    fn defaults_schedule_when_only_instruction() {
        assert_eq!(
            parse_heartbeat_command("check logs").unwrap(),
            ParsedHeartbeatCommand::Set {
                schedule: DEFAULT_HEARTBEAT_SCHEDULE.into(),
                instruction: "check logs".into(),
                delivery_mode: ScheduleDeliveryMode::Steer,
            }
        );
    }

    /// Regression: a bare leading duration used to be swallowed into the
    /// instruction with the 5m default cadence.
    #[test]
    fn parses_bare_leading_duration_as_schedule() {
        assert_eq!(
            parse_heartbeat_command("40s check the build").unwrap(),
            ParsedHeartbeatCommand::Set {
                schedule: "every 40s".into(),
                instruction: "check the build".into(),
                delivery_mode: ScheduleDeliveryMode::Steer,
            }
        );
        // A bare number followed by a unit word reads as prose, not a
        // schedule: it stays in the instruction with the default cadence.
        assert_eq!(
            parse_heartbeat_command("5 minutes remaining, warn me").unwrap(),
            ParsedHeartbeatCommand::Set {
                schedule: DEFAULT_HEARTBEAT_SCHEDULE.into(),
                instruction: "5 minutes remaining, warn me".into(),
                delivery_mode: ScheduleDeliveryMode::Steer,
            }
        );
        // A duration alone is not a heartbeat (no instruction).
        assert!(parse_heartbeat_command("40s").is_err());
    }

    #[test]
    fn parses_follow_up_flag() {
        assert_eq!(
            parse_heartbeat_command("--follow-up every 1m ping").unwrap(),
            ParsedHeartbeatCommand::Set {
                schedule: "every 1m".into(),
                instruction: "ping".into(),
                delivery_mode: ScheduleDeliveryMode::FollowUp,
            }
        );
    }

    #[test]
    fn run_set_pause_resume_clear_round_trip() {
        let dir = tempdir().expect("tempdir");
        let store = ScheduleStore::new(dir.path());
        let session_id = SessionId::from_string("ses_hb_cmd".into());

        let set = run_heartbeat_command(
            &store,
            SessionHeartbeatCommandParams {
                session_id,
                cwd: None,
                args: "every 30s reply PONG".into(),
            },
        )
        .expect("set");
        assert_eq!(set.action, HeartbeatCommandAction::Set);
        let job = set.job.expect("job");
        assert_eq!(job.instruction.as_deref(), Some("reply PONG"));
        assert_eq!(job.schedule.as_deref(), Some("every 30s"));
        assert_eq!(job.interval_ms, Some(30_000));

        let paused = run_heartbeat_command(
            &store,
            SessionHeartbeatCommandParams {
                session_id,
                cwd: None,
                args: "pause".into(),
            },
        )
        .expect("pause");
        assert_eq!(
            paused.job.as_ref().map(|j| j.status),
            Some(ScheduleJobStatus::Paused)
        );

        let resumed = run_heartbeat_command(
            &store,
            SessionHeartbeatCommandParams {
                session_id,
                cwd: None,
                args: "resume".into(),
            },
        )
        .expect("resume");
        assert_eq!(
            resumed.job.as_ref().map(|j| j.status),
            Some(ScheduleJobStatus::Active)
        );

        let cleared = run_heartbeat_command(
            &store,
            SessionHeartbeatCommandParams {
                session_id,
                cwd: None,
                args: "clear".into(),
            },
        )
        .expect("clear");
        assert_eq!(cleared.action, HeartbeatCommandAction::Clear);

        let status = run_heartbeat_command(
            &store,
            SessionHeartbeatCommandParams {
                session_id,
                cwd: None,
                args: String::new(),
            },
        )
        .expect("status");
        assert!(status.job.is_none());
    }
}
