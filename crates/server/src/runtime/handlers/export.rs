//! Native `session/export` and `session/import` handlers.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;

use crate::ProtocolErrorCode;
use crate::SuccessResponse;
use crate::runtime::ServerRuntime;

impl ServerRuntime {
    pub(crate) async fn handle_native_session_export(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_schedule::SessionExportParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/export params: {error}"),
                    );
                }
            };
        let session_id = params.session_id;
        let rollout_path = self
            .deps
            .db
            .get_session_index(&session_id)
            .ok()
            .flatten()
            .and_then(|index| index.rollout_path)
            .or_else(|| {
                self.rollout_store
                    .find_rollout_by_session_id(&session_id)
                    .ok()
                    .flatten()
            });
        let Some(rollout_path) = rollout_path else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                format!("session rollout not found: {}", session_id.as_str()),
            );
        };
        let default_ext = match params.format {
            devo_protocol::native::rpc_schedule::SessionExportFormat::Jsonl => "jsonl",
            devo_protocol::native::rpc_schedule::SessionExportFormat::Html => "html",
        };
        let export_dir = self.metadata.server_home.join("exports");
        if let Err(error) = std::fs::create_dir_all(&export_dir) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to create export dir: {error}"),
            );
        }
        let session_cwd = self
            .deps
            .db
            .get_session_index(&session_id)
            .ok()
            .flatten()
            .map(|index| index.session.cwd);
        let out_path = match params.path {
            Some(path) if path.is_relative() => session_cwd
                .as_ref()
                .map(|cwd| cwd.join(&path))
                .unwrap_or(path),
            Some(path) => path,
            None => export_dir.join(format!(
                "{}-{}.{}",
                session_id.as_str(),
                Utc::now().format("%Y%m%dT%H%M%SZ"),
                default_ext
            )),
        };
        let mut allowed_roots: Vec<&Path> =
            vec![self.metadata.server_home.as_path(), export_dir.as_path()];
        if let Some(cwd) = session_cwd.as_ref() {
            allowed_roots.push(cwd.as_path());
        }
        if let Err(error) = ensure_path_under_allowed_roots(&out_path, &allowed_roots) {
            return self.error_response(request_id, ProtocolErrorCode::PermissionDenied, error);
        }
        if let Some(parent) = out_path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to create export parent: {error}"),
            );
        }
        let result = match params.format {
            devo_protocol::native::rpc_schedule::SessionExportFormat::Jsonl => {
                std::fs::copy(&rollout_path, &out_path).map(|_| ())
            }
            devo_protocol::native::rpc_schedule::SessionExportFormat::Html => {
                export_rollout_as_html(&rollout_path, &out_path, session_id.as_str())
            }
        };
        if let Err(error) = result {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to export session: {error}"),
            );
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_schedule::SessionExportResult { path: out_path },
        })
        .expect("serialize session/export")
    }

    pub(crate) async fn handle_native_session_import(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_schedule::SessionImportParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/import params: {error}"),
                    );
                }
            };
        let cwd = params
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let import_path = resolve_import_path(&params.path, &cwd);
        let exports_dir = self.metadata.server_home.join("exports");
        let sessions_dir = self.metadata.server_home.join("sessions");
        let artifacts_dir = self.metadata.server_home.join("session-artifacts");
        if let Err(error) = ensure_path_under_allowed_roots(
            &import_path,
            &[
                self.metadata.server_home.as_path(),
                exports_dir.as_path(),
                sessions_dir.as_path(),
                artifacts_dir.as_path(),
            ],
        ) {
            // Also allow under the provided cwd when present.
            if ensure_path_under_allowed_roots(&import_path, &[&cwd]).is_err() {
                return self.error_response(request_id, ProtocolErrorCode::PermissionDenied, error);
            }
        }
        if !import_path.is_file() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("import path is not a file: {}", import_path.display()),
            );
        }
        let import_jsonl = match read_valid_import_jsonl(&import_path) {
            Ok(contents) => contents,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("session/import requires a valid v2 rollout: {error}"),
                );
            }
        };
        let create = self
            .handle_native_session_new(
                connection_id,
                serde_json::json!(null),
                serde_json::json!({
                    "cwd": cwd,
                    "idempotencyKey": format!("import:{}", Uuid::now_v7()),
                }),
            )
            .await;
        let session_id = match create
            .get("result")
            .and_then(|r| r.get("session"))
            .and_then(|s| s.get("id"))
            .and_then(|id| id.as_str())
        {
            Some(id) => devo_protocol::native::ids::SessionId::from_string(id.to_string()),
            None => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("session/import failed to create session: {create}"),
                );
            }
        };
        let dest = match self
            .deps
            .db
            .get_session_index(&session_id)
            .ok()
            .flatten()
            .and_then(|index| index.rollout_path)
            .or_else(|| {
                self.rollout_store
                    .find_rollout_by_session_id(&session_id)
                    .ok()
                    .flatten()
            }) {
            Some(path) => path,
            None => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    "imported session has no rollout path",
                );
            }
        };
        // Drop the empty session/new actor before rewriting the rollout so
        // shutdown cannot race or overwrite appended import lines.
        self.runtime_arc()
            .evict_parent_session_cascade(session_id)
            .await;
        if let Err(error) = append_import_jsonl(&import_jsonl, &dest, session_id.as_str()) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to append import jsonl: {error}"),
            );
        }
        if let Err(error) = self.rollout_store.index_rollout_metadata(&self.deps.db) {
            tracing::warn!(error = %error, "reindex after session/import failed");
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_schedule::SessionImportResult { session_id },
        })
        .expect("serialize session/import")
    }
}

use uuid::Uuid;

fn ensure_path_under_allowed_roots(path: &Path, roots: &[&Path]) -> Result<(), String> {
    let canonical = canonicalize_best_effort(path);
    for root in roots {
        let root_canon = canonicalize_best_effort(root);
        if canonical.starts_with(&root_canon) {
            return Ok(());
        }
    }
    Err(format!("path escapes allowed roots: {}", path.display()))
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    // For not-yet-created files, canonicalize the parent and rejoin the name.
    if let Some(parent) = path.parent()
        && let Ok(parent_canon) = parent.canonicalize()
        && let Some(name) = path.file_name()
    {
        return parent_canon.join(name);
    }
    normalize_lexically(path)
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn export_rollout_as_html(
    rollout_path: &Path,
    out_path: &Path,
    session_id: &str,
) -> std::io::Result<()> {
    let raw = std::fs::read_to_string(rollout_path)?;
    let mut body = String::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let escaped = html_escape(line);
        body.push_str(&format!("<pre>{escaped}</pre>\n"));
    }
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Session {session_id}</title></head><body><h1>Session {session_id}</h1>\n{body}</body></html>"
    );
    std::fs::write(out_path, html)
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn resolve_import_path(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn read_valid_import_jsonl(source: &Path) -> anyhow::Result<String> {
    let source_raw = std::fs::read_to_string(source)?;
    validate_import_jsonl(&source_raw)?;
    Ok(source_raw)
}

fn validate_import_jsonl(source_raw: &str) -> anyhow::Result<()> {
    for (line_index, line) in source_raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        devo_core::parse_rollout_line(trimmed)
            .with_context(|| format!("invalid rollout line {}", line_index + 1))?;
    }
    Ok(())
}

fn append_import_jsonl(source_raw: &str, dest: &Path, dest_session_id: &str) -> anyhow::Result<()> {
    validate_import_jsonl(source_raw)?;
    let old_session_id = source_raw.lines().find_map(extract_session_meta_id);
    let mut dest_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dest)?;
    use std::io::Write;
    for line in source_raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Skip SessionMeta from the import file — destination already has
        // its own meta line from session/new.
        if is_session_meta_line(trimmed) {
            continue;
        }
        // Workspace artifact lines point at the source session's on-disk
        // snapshots and cannot be revived by a JSONL copy alone.
        if is_workspace_artifact_line(trimmed) {
            continue;
        }
        let rewritten =
            rewrite_import_line_session_id(trimmed, old_session_id.as_deref(), dest_session_id)?;
        writeln!(dest_file, "{rewritten}")?;
    }
    Ok(())
}

fn extract_session_meta_id(line: &str) -> Option<String> {
    if !is_session_meta_line(line) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value
        .pointer("/session/id")
        .or_else(|| value.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn is_workspace_artifact_line(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    matches!(
        value.get("kind").and_then(|kind| kind.as_str()),
        Some(
            "workspaceCheckpoint"
                | "workspaceChange"
                | "workspaceRestoreStarted"
                | "workspaceRestoreCompleted"
        )
    )
}

fn rewrite_import_line_session_id(
    line: &str,
    old_session_id: Option<&str>,
    dest_session_id: &str,
) -> anyhow::Result<String> {
    let mut value: serde_json::Value = serde_json::from_str(line)
        .with_context(|| format!("import jsonl line is not valid JSON: {line}"))?;
    if let Some(obj) = value.as_object_mut() {
        if obj.contains_key("sessionId") {
            obj.insert(
                "sessionId".to_string(),
                serde_json::Value::String(dest_session_id.to_string()),
            );
        }
        if obj.contains_key("session_id") {
            obj.insert(
                "session_id".to_string(),
                serde_json::Value::String(dest_session_id.to_string()),
            );
        }
    }
    if let Some(old_session_id) =
        old_session_id.filter(|id| !id.is_empty() && *id != dest_session_id)
    {
        replace_session_id_strings(&mut value, old_session_id, dest_session_id);
    }
    Ok(serde_json::to_string(&value)?)
}

fn replace_session_id_strings(value: &mut serde_json::Value, old_id: &str, new_id: &str) {
    match value {
        serde_json::Value::String(text) => {
            if text == old_id || text.contains(old_id) {
                *text = text.replace(old_id, new_id);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                replace_session_id_strings(item, old_id, new_id);
            }
        }
        serde_json::Value::Object(map) => {
            for nested in map.values_mut() {
                replace_session_id_strings(nested, old_id, new_id);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn is_session_meta_line(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    value.get("kind").and_then(|kind| kind.as_str()) == Some("sessionMeta")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    #[test]
    fn resolve_import_path_joins_relative_against_cwd() {
        let cwd = PathBuf::from("/tmp/project");
        assert_eq!(
            resolve_import_path(Path::new("out.jsonl"), &cwd),
            PathBuf::from("/tmp/project/out.jsonl")
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolve_import_path_keeps_absolute_windows() {
        let cwd = PathBuf::from(r"C:\tmp\project");
        assert_eq!(
            resolve_import_path(Path::new(r"D:\exports\a.jsonl"), &cwd),
            PathBuf::from(r"D:\exports\a.jsonl")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_import_path_keeps_absolute_unix() {
        let cwd = PathBuf::from("/tmp/project");
        assert_eq!(
            resolve_import_path(Path::new("/exports/a.jsonl"), &cwd),
            PathBuf::from("/exports/a.jsonl")
        );
    }

    #[test]
    fn is_session_meta_line_requires_native_kind_tag() {
        assert!(is_session_meta_line(r#"{"kind":"sessionMeta","v":2}"#));
        assert!(!is_session_meta_line(r#"{"type":"sessionMeta","id":"x"}"#));
        assert!(!is_session_meta_line(r#"{"kind":"item","v":2}"#));
    }

    #[test]
    fn append_import_jsonl_skips_source_meta_and_keeps_dest_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("source.jsonl");
        let dest = dir.path().join("dest.jsonl");
        let now = Utc::now();
        let old_session_id = devo_protocol::native::ids::SessionId::from_string("ses_old".into());
        let source_session = devo_protocol::native::session::Session {
            id: old_session_id,
            version: 1,
            cwd: std::env::current_dir().expect("current directory"),
            additional_directories: Vec::new(),
            parent: None,
            fork_from_id: None,
            at_turn_id: None,
            ephemeral: false,
            created_at: now,
            status: devo_protocol::native::session::SessionStatus::Idle,
            flags: Vec::new(),
            archived: false,
            activity: devo_protocol::native::session::SessionActivity::Idle,
            active_turn_id: None,
            queued_count: 0,
            title: None,
            title_state: devo_core::SessionTitleState::Unset,
            model: devo_protocol::native::model::ModelBinding {
                provider: "test".into(),
                model: "test".into(),
                variant: None,
                reasoning_effort: None,
            },
            settings: devo_protocol::native::session::SessionSettings {
                permission_profile: devo_protocol::native::model::PermissionProfile::AutoReview,
                reasoning_effort: None,
                mode: None,
                sandbox_profile: None,
                effective_context_window: None,
                auto_refine_enabled: None,
                auto_refine_turn_interval: None,
                python_cell_first_wait_ms: None,
            },
            git_info: None,
            preview: String::new(),
            last_activity_at: now,
            transcript_size_bytes: None,
            message_count: None,
            summary: None,
            task_state: None,
            usage: devo_protocol::native::usage::SessionUsage {
                total: Default::default(),
                by_purpose: Vec::new(),
                legacy: None,
                updated_at: now,
            },
        };
        let source_lines = [
            serde_json::to_string(&devo_core::session_line(source_session.clone(), None, now))
                .expect("source session line"),
            serde_json::to_string(&devo_core::title_line(
                now,
                old_session_id,
                "first".into(),
                None,
            ))
            .expect("first title line"),
            serde_json::to_string(&devo_core::title_line(
                now,
                old_session_id,
                "second".into(),
                None,
            ))
            .expect("second title line"),
        ];
        std::fs::write(&source, source_lines.join("\n") + "\n").expect("write source");
        let destination_session = devo_protocol::native::session::Session {
            id: devo_protocol::native::ids::SessionId::from_string("ses_new".into()),
            ..source_session
        };
        let destination_meta =
            serde_json::to_string(&devo_core::session_line(destination_session, None, now))
                .expect("destination session line");
        std::fs::write(&dest, destination_meta + "\n").expect("write dest");

        let source_raw = read_valid_import_jsonl(&source).expect("valid source rollout");
        append_import_jsonl(&source_raw, &dest, "ses_new").expect("append");

        let body = std::fs::read_to_string(&dest).expect("read dest");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3);
        let title: serde_json::Value = serde_json::from_str(lines[2]).expect("title json");
        assert_eq!(
            title.get("sessionId").and_then(|v| v.as_str()),
            Some("ses_new")
        );
    }

    #[test]
    fn append_import_jsonl_rejects_legacy_before_writing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("dest.jsonl");
        let original = "destination remains unchanged\n";
        std::fs::write(&dest, original).expect("write destination");
        let legacy = r#"{"type":"session_title_updated","title":"old"}"#;

        let error = append_import_jsonl(legacy, &dest, "ses_new").expect_err("reject legacy");

        assert!(error.to_string().contains("invalid rollout line 1"));
        assert_eq!(
            std::fs::read_to_string(dest).expect("read destination"),
            original
        );
    }
}
