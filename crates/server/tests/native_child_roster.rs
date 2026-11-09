//! Read-only, parent-scoped Native discovery used by the Desktop subagent roster.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use devo_protocol::SessionTitleState;
use devo_protocol::native::ids::SessionId;
use devo_protocol::native::page::Page;
use devo_protocol::native::session::Session;
use devo_server::db::{Database, SessionIndexRow};
use devo_server::test_support::TestRuntime;
use devo_server::{ClientTransportKind, ServerRuntime};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

fn row(cwd: &Path, id: &str, parent: Option<SessionId>) -> SessionIndexRow {
    let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    SessionIndexRow {
        session_id: id.into(),
        cwd: cwd.to_path_buf(),
        additional_directories: Vec::new(),
        created_at: now,
        updated_at: now,
        last_activity_at: now,
        title: Some(id.to_string()),
        title_state: SessionTitleState::Unset,
        parent_session_id: parent,
        fork_from_id: None,
        fork_at_turn_id: None,
        agent_path: parent.map(|_| format!("root/{id}")),
        ephemeral: false,
        model: None,
        reasoning_effort_selection: None,
    }
}

async fn request(
    runtime: &Arc<ServerRuntime>,
    connection: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let response = runtime
        .handle_incoming(
            connection,
            json!({
                "id": 1, "method": method, "params": params,
            }),
        )
        .await
        .context("missing RPC response")?;
    anyhow::ensure!(response.get("error").is_none(), "{method}: {response}");
    Ok(response["result"].clone())
}

async fn connect(runtime: &Arc<ServerRuntime>) -> Result<u64> {
    let (outbound, _incoming) = devo_server::test_outbound_channel(4096);
    let connection = runtime
        .register_connection(ClientTransportKind::Stdio, outbound)
        .await;
    request(
        runtime,
        connection,
        "initialize",
        json!({
            "protocolVersion": 1, "clientCapabilities": {},
            "_meta": { "devo": { "protocol": "native" } },
        }),
    )
    .await?;
    Ok(connection)
}

#[tokio::test]
async fn filters_direct_children_before_pagination_without_reading_rollouts() -> Result<()> {
    let home = tempfile::tempdir()?;
    let db = Arc::new(Database::open(home.path().join("index.db"))?);
    let runtime = TestRuntime::noop()
        .database(db.clone())
        .runtime(home.path());
    let connection = connect(&runtime).await?;
    let parent: SessionId = "parent".into();
    let other: SessionId = "other".into();
    // More unrelated rows than Desktop's entire five-page scan. Tied timestamps
    // also exercise deterministic ordering rather than HashMap iteration order.
    for number in 0..1_010 {
        db.upsert_session(
            row(home.path(), &format!("aaa-noise-{number:04}"), Some(other)),
            /*rollout_path*/ None,
        )?;
    }
    let mut expected = Vec::new();
    for id in ["child-a", "child-b", "child-c"] {
        let mut child = row(home.path(), id, Some(parent));
        if id == "child-c" {
            child.cwd = home.path().join("different-worktree");
        }
        // Nonexistent rollout paths prove list does not require hydration.
        db.upsert_session(
            &child,
            Some(&home.path().join(format!("{id}-missing.jsonl"))),
        )?;
        let mut native = child.into_native_session();
        native.task_state = Some("idle".to_string());
        expected.push(native);
    }
    let mut fork = row(home.path(), "fork", Some(parent));
    fork.agent_path = None; // Historical forks may still have parent_session_id.
    db.upsert_session(fork, /*rollout_path*/ None)?;
    let mut side = row(home.path(), "ephemeral", Some(parent));
    side.ephemeral = true;
    db.upsert_session(side, /*rollout_path*/ None)?;
    db.upsert_session(
        row(home.path(), "grandchild", Some("child-a".into())),
        /*rollout_path*/ None,
    )?;
    db.upsert_session(
        row(home.path(), "root", /*parent*/ None),
        /*rollout_path*/ None,
    )?;

    for include in [None, Some(false), Some(true)] {
        let mut params = json!({"parentSessionId": parent, "limit": 2});
        if let Some(include) = include {
            params["includeChildren"] = json!(include);
        }
        let first: Page<Session> = serde_json::from_value(
            request(&runtime, connection, "session/list", params.clone()).await?,
        )?;
        assert_eq!(
            first,
            Page {
                data: expected[..2].to_vec(),
                next_cursor: Some("2".to_string())
            }
        );
        params["cursor"] = json!("2");
        let second: Page<Session> =
            serde_json::from_value(request(&runtime, connection, "session/list", params).await?)?;
        assert_eq!(
            second,
            Page {
                data: expected[2..].to_vec(),
                next_cursor: None
            }
        );
    }
    let filtered: Page<Session> = serde_json::from_value(request(&runtime, connection, "session/list", json!({
        "parentSessionId": parent, "cwds": [home.path().join("different-worktree")], "search": "CHILD-C",
    })).await?)?;
    assert_eq!(
        filtered,
        Page {
            data: expected[2..].to_vec(),
            next_cursor: None
        }
    );
    let missing: Page<Session> = serde_json::from_value(
        request(
            &runtime,
            connection,
            "session/list",
            json!({
                "parentSessionId": "missing",
            }),
        )
        .await?,
    )?;
    assert_eq!(
        missing,
        Page {
            data: vec![],
            next_cursor: None
        }
    );
    // The existing root-only call still returns roots/forks, never subagents.
    let roots: Page<Session> =
        serde_json::from_value(request(&runtime, connection, "session/list", json!({})).await?)?;
    assert_eq!(
        roots
            .data
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fork", "root"]
    );
    assert!(!home.path().join("session-artifacts").exists());
    Ok(())
}

#[path = "support/subagent_lifecycle.rs"]
mod support;

#[tokio::test]
async fn scoped_roster_preserves_live_child_status_and_parent_scope() -> Result<()> {
    let home = tempfile::tempdir()?;
    let provider = Arc::new(support::ScriptedProvider::pending());
    let runtime = support::build_runtime(home.path(), provider.clone())?;
    let (connection, mut notifications) = support::initialize_connection(&runtime).await?;
    let parent = support::start_parent_session(&runtime, connection, home.path()).await?;
    let child = support::spawn_child_named(
        &runtime,
        parent,
        "Run child work",
        /*fork_turns*/ None,
        Some("factory-test-work"),
    )
    .await?;
    support::wait_for_child_turn_started(&mut notifications, child.child_session_id).await?;
    let result: Page<Session> = serde_json::from_value(
        request(
            &runtime,
            connection,
            "session/list",
            json!({
                "parentSessionId": parent,
            }),
        )
        .await?,
    )?;
    assert_eq!(
        result
            .data
            .iter()
            .map(|session| (
                session.id,
                session.parent.clone(),
                session.status,
                session.activity
            ))
            .collect::<Vec<_>>(),
        vec![(
            child.child_session_id,
            Some(devo_protocol::native::session::SessionParent::Agent {
                session_id: parent,
                role: Some("default".to_string())
            }),
            devo_protocol::native::session::SessionStatus::Active,
            devo_protocol::native::session::SessionActivity::Working,
        )]
    );
    assert!(result.data[0].active_turn_id.is_some());
    let unknown: Page<Session> = serde_json::from_value(
        request(
            &runtime,
            connection,
            "session/list",
            json!({
                "parentSessionId": "unrelated-parent",
            }),
        )
        .await?,
    )?;
    assert_eq!(
        unknown,
        Page {
            data: vec![],
            next_cursor: None
        }
    );
    request(
        &runtime,
        connection,
        "session/interrupt",
        json!({
            "scope": {"scope": "session", "sessionId": child.child_session_id},
        }),
    )
    .await?;
    support::wait_for_session_notification(
        &mut notifications,
        "turn/completed",
        child.child_session_id,
    )
    .await?;
    Ok(())
}
