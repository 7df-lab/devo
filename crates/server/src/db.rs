use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use rusqlite::{Connection, params, types::Type};
use serde_json;

use devo_protocol::native::ids::{SessionId, TurnId};
use devo_protocol::native::item::ContextOccupancy;
use devo_protocol::{PendingInputItem, PendingInputKind, QueueItemId, SessionTitleState};

/// Queue type for pending messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueType {
    /// Pending turn inputs (from turn/start while a turn is active).
    Turn,
    /// Inputs injected into the active turn by `turn/steer`.
    Steer,
}

impl QueueType {
    fn as_str(&self) -> &'static str {
        match self {
            QueueType::Turn => "turn",
            QueueType::Steer => "steer",
        }
    }
}

/// SQLite's deliberately narrow session-index model.
///
/// This is not a first-party runtime session model. It contains only columns
/// stored by the rebuildable SQLite index; callers must convert it to Native
/// `Session` (or the legacy ACP adapter) at this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIndexRow {
    pub session_id: SessionId,
    pub cwd: PathBuf,
    pub additional_directories: Vec<PathBuf>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
    pub last_activity_at: chrono::DateTime<Utc>,
    pub title: Option<String>,
    pub title_state: SessionTitleState,
    pub parent_session_id: Option<SessionId>,
    pub fork_from_id: Option<SessionId>,
    pub fork_at_turn_id: Option<TurnId>,
    pub agent_path: Option<String>,
    pub ephemeral: bool,
    pub model: Option<String>,
    pub reasoning_effort_selection: Option<String>,
}

impl SessionIndexRow {
    /// Projects this rebuildable index row into the first-party Native shape.
    pub fn into_native_session(self) -> devo_protocol::native::session::Session {
        use devo_protocol::native::model::{ModelBinding, PermissionProfile};
        use devo_protocol::native::session::{
            Session, SessionActivity, SessionParent, SessionSettings, SessionStatus,
        };
        use devo_protocol::native::usage::{SessionUsage, UsageTotals};

        let has_agent_parent = self.agent_path.is_some();
        let parent = self.parent_session_id.and_then(|session_id| {
            has_agent_parent.then_some(SessionParent::Agent {
                session_id,
                role: None,
            })
        });
        let fork_from_id = self.fork_from_id.or_else(|| {
            (!has_agent_parent)
                .then_some(self.parent_session_id)
                .flatten()
        });
        Session {
            id: self.session_id,
            version: 1,
            cwd: self.cwd,
            additional_directories: self.additional_directories,
            parent,
            fork_from_id,
            at_turn_id: self.fork_at_turn_id,
            ephemeral: self.ephemeral,
            created_at: self.created_at,
            status: SessionStatus::Idle,
            flags: Vec::new(),
            archived: false,
            activity: SessionActivity::Idle,
            active_turn_id: None,
            queued_count: 0,
            title: self.title,
            title_state: self.title_state,
            model: ModelBinding {
                provider: "unknown".into(),
                model: self.model.unwrap_or_default(),
                variant: None,
                reasoning_effort: self
                    .reasoning_effort_selection
                    .as_deref()
                    .and_then(|selection| selection.parse().ok()),
            },
            settings: SessionSettings {
                permission_profile: PermissionProfile::AutoReview,
                reasoning_effort: self
                    .reasoning_effort_selection
                    .as_deref()
                    .map(devo_protocol::normalize_reasoning_effort_selection_for_ui),
                mode: Some(String::new()),
                sandbox_profile: None,
                effective_context_window: None,
                auto_refine_enabled: None,
                auto_refine_turn_interval: None,
                python_cell_first_wait_ms: None,
            },
            git_info: None,
            preview: String::new(),
            last_activity_at: self.last_activity_at,
            transcript_size_bytes: None,
            message_count: None,
            summary: None,
            task_state: None,
            usage: SessionUsage {
                total: UsageTotals::default(),
                by_purpose: Vec::new(),
                legacy: None,
                updated_at: self.updated_at,
            },
        }
    }
}

impl From<&SessionIndexRow> for SessionIndexRow {
    fn from(row: &SessionIndexRow) -> Self {
        row.clone()
    }
}

impl From<&devo_protocol::native::session::Session> for SessionIndexRow {
    fn from(session: &devo_protocol::native::session::Session) -> Self {
        let parent_session_id = session.parent.as_ref().map(|parent| match parent {
            devo_protocol::native::session::SessionParent::Agent { session_id, .. } => *session_id,
        });
        Self {
            session_id: session.id,
            cwd: session.cwd.clone(),
            additional_directories: session.additional_directories.clone(),
            created_at: session.created_at,
            updated_at: session.usage.updated_at,
            last_activity_at: session.last_activity_at,
            title: session.title.clone(),
            title_state: session.title_state.clone(),
            parent_session_id,
            fork_from_id: session.fork_from_id,
            fork_at_turn_id: session.at_turn_id,
            agent_path: session.parent.as_ref().map(|_| "subagent".to_string()),
            ephemeral: session.ephemeral,
            model: (!session.model.model.is_empty()).then(|| session.model.model.clone()),
            reasoning_effort_selection: session.settings.reasoning_effort.clone(),
        }
    }
}

impl From<&crate::runtime_session_summary::RuntimeSessionSummary> for SessionIndexRow {
    fn from(summary: &crate::runtime_session_summary::RuntimeSessionSummary) -> Self {
        let fork_from_id = summary.native.fork_from_id;
        Self {
            session_id: summary.session_id(),
            cwd: summary.native.cwd.clone(),
            additional_directories: summary.native.additional_directories.clone(),
            created_at: summary.native.created_at,
            updated_at: summary.updated_at,
            last_activity_at: summary.native.last_activity_at,
            title: summary.native.title.clone(),
            title_state: summary.native.title_state.clone(),
            parent_session_id: summary.parent_session_id().or(fork_from_id),
            fork_from_id,
            fork_at_turn_id: summary.native.at_turn_id,
            agent_path: summary.agent_path.clone(),
            ephemeral: summary.native.ephemeral,
            model: summary.model_name().map(ToOwned::to_owned),
            reasoning_effort_selection: summary.native.settings.reasoning_effort.clone(),
        }
    }
}

/// SQLite index row plus the rollout locator used for lazy resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIndexRecord {
    pub session: SessionIndexRow,
    pub rollout_path: Option<PathBuf>,
}

/// Source of a session metadata upsert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionUpsertSource {
    /// Live runtime writes should preserve existing non-null fields when the update omits them.
    RuntimeLive,
    /// Rollout index writes rebuild SQLite from canonical rollout metadata.
    RolloutIndex,
}

/// Session-level token statistics.
#[derive(Debug, Clone)]
pub struct SessionStats {
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub total_tokens: usize,
    pub total_cache_creation_tokens: usize,
    pub total_cache_read_tokens: usize,
    /// Latest **model-query** input tokens (not cumulative turn usage).
    ///
    /// Used for hydrate / diagnostics. Must not be confused with turn-aggregate
    /// usage on a completed turn, which sums every completed leg in a multi-tool turn.
    pub last_input_tokens: usize,
    pub turn_count: usize,
    pub prompt_token_estimate: usize,
    /// Latest context-window occupancy breakdown, when known.
    pub last_context_occupancy: Option<ContextOccupancy>,
}

/// One derived event row before per-stream sequencing (08 §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEventLogRow {
    /// Stable identity of the rollout fact: `<rollout_path>#<line_index>`.
    pub source_fact_id: String,
    /// Notification method, e.g. `item/started`.
    pub event_kind: String,
    pub stream_id: String,
    pub event_id: String,
    /// `EventEnvelope` JSON (meta + notification).
    pub payload: String,
    pub created_at: String,
}

/// A stored event row including its per-stream sequence number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLogRow {
    pub source_fact_id: String,
    pub event_kind: String,
    pub stream_id: String,
    pub event_id: String,
    pub seq: u64,
    pub payload: String,
    pub created_at: String,
}

/// Current index schema version recorded in `schema_meta` (05 §2.3).
/// Bump when a migration changes the index layout; the rollout files are
/// the rebuildable source of truth on any mismatch.
/// v2: adds `event_log` + `projection_watermark` (08 §5/§7).
/// v3: renames the persisted active-turn steer queue from `btw` to `steer`.
const CURRENT_SCHEMA_VERSION: u32 = 3;

/// SQLite database for session metadata, token stats, and pending queues.
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Opens or creates the SQLite database at the given path.
    pub fn open(db_path: PathBuf) -> Result<Self> {
        let conn = Connection::open(&db_path)
            .with_context(|| format!("failed to open database at {}", db_path.display()))?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Runs schema migrations.
    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                title TEXT,
                title_state TEXT NOT NULL DEFAULT 'unset',
                model TEXT,
                thinking TEXT,
                cwd TEXT NOT NULL,
                additional_directories TEXT NOT NULL DEFAULT '[]',
                ephemeral INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_activity_at INTEGER NOT NULL DEFAULT 0,
                schema_version INTEGER NOT NULL DEFAULT 3,
                rollout_path TEXT,
                parent_session_id TEXT,
                fork_from_id TEXT,
                fork_at_turn_id TEXT,
                agent_path TEXT
            );

            CREATE TABLE IF NOT EXISTS session_stats (
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                last_input_tokens INTEGER NOT NULL DEFAULT 0,
                turn_count INTEGER NOT NULL DEFAULT 0,
                prompt_token_estimate INTEGER NOT NULL DEFAULT 0,
                last_context_occupancy TEXT
            );

            CREATE TABLE IF NOT EXISTS pending_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                queue_type TEXT NOT NULL CHECK(queue_type IN ('turn', 'steer')),
                kind TEXT NOT NULL,
                content TEXT NOT NULL,
                pending_input_id TEXT,
                metadata TEXT,
                created_at INTEGER NOT NULL,
                position INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_pending_session
                ON pending_messages(session_id, queue_type);
            ",
        )
        .context("failed to run database migrations")?;
        for (table, column, alter_sql) in [
            (
                "sessions",
                "additional_directories",
                "ALTER TABLE sessions ADD COLUMN additional_directories TEXT NOT NULL DEFAULT '[]'",
            ),
            (
                "pending_messages",
                "pending_input_id",
                "ALTER TABLE pending_messages ADD COLUMN pending_input_id TEXT",
            ),
            (
                "session_stats",
                "last_context_occupancy",
                "ALTER TABLE session_stats ADD COLUMN last_context_occupancy TEXT",
            ),
            (
                "sessions",
                "rollout_path",
                "ALTER TABLE sessions ADD COLUMN rollout_path TEXT",
            ),
            (
                "sessions",
                "parent_session_id",
                "ALTER TABLE sessions ADD COLUMN parent_session_id TEXT",
            ),
            (
                "sessions",
                "agent_path",
                "ALTER TABLE sessions ADD COLUMN agent_path TEXT",
            ),
            (
                "sessions",
                "fork_from_id",
                "ALTER TABLE sessions ADD COLUMN fork_from_id TEXT",
            ),
            (
                "sessions",
                "fork_at_turn_id",
                "ALTER TABLE sessions ADD COLUMN fork_at_turn_id TEXT",
            ),
        ] {
            ensure_column(&conn, table, column, alter_sql)?;
        }
        if ensure_column(
            &conn,
            "sessions",
            "last_activity_at",
            "ALTER TABLE sessions ADD COLUMN last_activity_at INTEGER NOT NULL DEFAULT 0",
        )? {
            conn.execute(
                "UPDATE sessions SET last_activity_at = updated_at WHERE last_activity_at = 0",
                [],
            )
            .context("failed to backfill last_activity_at column")?;
        }
        if ensure_column(
            &conn,
            "session_stats",
            "total_tokens",
            "ALTER TABLE session_stats ADD COLUMN total_tokens INTEGER NOT NULL DEFAULT 0",
        )? {
            conn.execute(
                "UPDATE session_stats SET total_tokens = total_input_tokens + total_output_tokens",
                [],
            )
            .context("failed to backfill total_tokens column")?;
        }
        // User forks used to reuse parent_session_id. Move those rows onto
        // fork_from_id so delete/list treat them as independent sessions.
        conn.execute(
            "UPDATE sessions
             SET fork_from_id = COALESCE(fork_from_id, parent_session_id),
                 parent_session_id = NULL
             WHERE parent_session_id IS NOT NULL
               AND agent_path IS NULL
               AND (fork_from_id IS NULL OR fork_from_id = '')",
            [],
        )
        .context("failed to migrate user-fork parent_session_id into fork_from_id")?;
        // Queue entries have an explicit position so `session/queue/update`
        // can reorder without rewriting row ids (P4c); existing rows keep
        // their insertion order (position = id).
        if ensure_column(
            &conn,
            "pending_messages",
            "position",
            "ALTER TABLE pending_messages ADD COLUMN position INTEGER",
        )? {
            conn.execute(
                "UPDATE pending_messages SET position = id WHERE position IS NULL",
                [],
            )
            .context("failed to backfill pending_messages position")?;
        }
        let pending_messages_sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'pending_messages'",
                [],
                |row| row.get(0),
            )
            .ok();
        if pending_messages_sql
            .as_deref()
            .is_some_and(|sql| sql.contains("'btw'"))
        {
            // P4c originally used `btw` for the active-turn steer queue. The
            // product `/btw` feature is an unrelated ephemeral side question,
            // so migrate the storage value and CHECK constraint together.
            conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE pending_messages RENAME TO pending_messages_legacy;
                CREATE TABLE pending_messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    queue_type TEXT NOT NULL CHECK(queue_type IN ('turn', 'steer')),
                    kind TEXT NOT NULL,
                    content TEXT NOT NULL,
                    pending_input_id TEXT,
                    metadata TEXT,
                    created_at INTEGER NOT NULL,
                    position INTEGER NOT NULL
                );
                INSERT INTO pending_messages
                    (id, session_id, queue_type, kind, content, pending_input_id, metadata, created_at, position)
                SELECT id, session_id,
                    CASE queue_type WHEN 'btw' THEN 'steer' ELSE queue_type END,
                    kind, content, pending_input_id, metadata, created_at, position
                FROM pending_messages_legacy;
                DROP TABLE pending_messages_legacy;
                CREATE INDEX idx_pending_session
                    ON pending_messages(session_id, queue_type);
                COMMIT;
                ",
            )
            .context("failed to migrate pending steer queue from btw to steer")?;
        }
        // Schema version table (05 §2.3): the new authority going forward.
        // The ad-hoc column probes above are the v0 baseline and keep working
        // for databases created before this table existed.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .context("failed to create schema_meta table")?;
        conn.execute(
            "INSERT INTO schema_meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [CURRENT_SCHEMA_VERSION.to_string()],
        )
        .context("failed to record schema version")?;
        // Persisted event log (08 §5/§7): the rollout JSONL is the canonical
        // recovery log; this table is the delivery log used for cursor replay.
        // Rows are idempotent by (source_fact_id, event_kind, stream_id);
        // `seq` is strictly increasing per stream. A database rebuild expires
        // all cursors and forces re-snapshot — rows are never modified.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS event_log (
                source_fact_id TEXT NOT NULL,
                event_kind TEXT NOT NULL,
                stream_id TEXT NOT NULL,
                event_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (source_fact_id, event_kind, stream_id)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS event_log_stream_seq
                ON event_log(stream_id, seq);

            CREATE TABLE IF NOT EXISTS projection_watermark (
                rollout_path TEXT PRIMARY KEY,
                last_line_index INTEGER NOT NULL
            );",
        )
        .context("failed to create event_log tables")?;
        Ok(())
    }

    /// The recorded schema version, if any. `None` means the database
    /// predates the `schema_meta` table (implicitly version 0).
    pub fn schema_version(&self) -> Result<Option<u32>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .ok();
        value
            .map(|value| {
                value
                    .parse::<u32>()
                    .context("invalid schema_version in schema_meta")
            })
            .transpose()
    }

    // === Event log (08 §5/§7) ===

    /// Idempotently inserts derived event rows. `seq` is allocated per stream
    /// inside the same statement, and the `(source_fact_id, event_kind,
    /// stream_id)` primary key makes re-projection of the same rollout fact a
    /// no-op — reconciliation never duplicates, only backfills. Returns the
    /// number of rows actually inserted.
    pub fn insert_event_log_rows(&self, rows: &[NewEventLogRow]) -> Result<usize> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut inserted = 0usize;
        for row in rows {
            let changes = conn
                .execute(
                    "INSERT OR IGNORE INTO event_log
                        (source_fact_id, event_kind, stream_id, event_id, seq, payload, created_at)
                     SELECT ?1, ?2, ?3, ?4,
                        (SELECT COALESCE(MAX(seq), 0) + 1 FROM event_log WHERE stream_id = ?3),
                        ?5, ?6",
                    params![
                        row.source_fact_id,
                        row.event_kind,
                        row.stream_id,
                        row.event_id,
                        row.payload,
                        row.created_at,
                    ],
                )
                .context("failed to insert event_log row")?;
            inserted += changes;
        }
        Ok(inserted)
    }

    /// Reads stored events of one stream after `after_seq`, ordered by seq
    /// (cursor replay, 08 §4).
    pub fn event_log_rows(&self, stream_id: &str, after_seq: u64) -> Result<Vec<EventLogRow>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT source_fact_id, event_kind, stream_id, event_id, seq, payload, created_at
                 FROM event_log WHERE stream_id = ?1 AND seq > ?2 ORDER BY seq",
            )
            .context("failed to prepare event_log read")?;
        let rows = stmt
            .query_map(params![stream_id, after_seq as i64], |row| {
                Ok(EventLogRow {
                    source_fact_id: row.get(0)?,
                    event_kind: row.get(1)?,
                    stream_id: row.get(2)?,
                    event_id: row.get(3)?,
                    seq: row.get::<_, i64>(4)? as u64,
                    payload: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .context("failed to read event_log rows")?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to decode event_log rows")
    }

    /// Total number of stored event rows (reconciliation tests).
    pub fn event_log_len(&self) -> Result<u64> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM event_log", [], |row| row.get(0))
            .context("failed to count event_log rows")?;
        Ok(count as u64)
    }

    /// The highest stored seq of one stream — the subscription barrier seq
    /// (08 §4). `None` when the stream has no rows yet.
    pub fn event_log_max_seq(&self, stream_id: &str) -> Result<Option<u64>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let value: Option<i64> = conn
            .query_row(
                "SELECT MAX(seq) FROM event_log WHERE stream_id = ?1",
                params![stream_id],
                |row| row.get(0),
            )
            .context("failed to read stream barrier seq")?;
        Ok(value.map(|value| value as u64))
    }

    /// The last rollout line index projected into `event_log` for a file.
    pub fn projection_watermark(&self, rollout_path: &Path) -> Result<Option<u64>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let value: Option<i64> = conn
            .query_row(
                "SELECT last_line_index FROM projection_watermark WHERE rollout_path = ?1",
                params![rollout_path.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .ok();
        Ok(value.map(|value| value as u64))
    }

    /// Advances the projection watermark for a rollout file.
    pub fn set_projection_watermark(
        &self,
        rollout_path: &Path,
        last_line_index: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute(
            "INSERT INTO projection_watermark (rollout_path, last_line_index) VALUES (?1, ?2)
             ON CONFLICT(rollout_path) DO UPDATE SET last_line_index = excluded.last_line_index",
            params![
                rollout_path.to_string_lossy().as_ref(),
                last_line_index as i64
            ],
        )
        .context("failed to update projection watermark")?;
        Ok(())
    }

    // === Session CRUD ===

    /// Inserts or updates a session's metadata and optional rollout index fields.
    pub fn upsert_session<T>(
        &self,
        session: T,
        rollout_path: Option<&std::path::Path>,
    ) -> Result<()>
    where
        T: Into<SessionIndexRow>,
    {
        self.upsert_session_with_source(
            session.into(),
            rollout_path,
            SessionUpsertSource::RuntimeLive,
        )
    }

    /// Inserts or updates session metadata using rollout-index semantics.
    pub fn upsert_rollout_index_session<T>(
        &self,
        session: T,
        rollout_path: Option<&std::path::Path>,
    ) -> Result<()>
    where
        T: Into<SessionIndexRow>,
    {
        self.upsert_session_with_source(
            session.into(),
            rollout_path,
            SessionUpsertSource::RolloutIndex,
        )
    }

    fn upsert_session_with_source(
        &self,
        meta: SessionIndexRow,
        rollout_path: Option<&std::path::Path>,
        source: SessionUpsertSource,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let additional_directories = serde_json::to_string(&meta.additional_directories)
            .context("failed to serialize session additional directories")?;
        let title_state_str = match &meta.title_state {
            SessionTitleState::Unset => "unset",
            SessionTitleState::Generating => "generating",
            SessionTitleState::Final(_) => "final",
        };
        let rollout_path_str = rollout_path.map(|path| path.to_string_lossy().into_owned());
        let parent_session_id = meta
            .parent_session_id
            .as_ref()
            .map(|id| id.as_str().to_owned());
        let fork_from_id = meta.fork_from_id.as_ref().map(|id| id.as_str().to_owned());
        let fork_at_turn_id = meta
            .fork_at_turn_id
            .as_ref()
            .map(|id| id.as_str().to_owned());
        let agent_path = meta.agent_path.clone();
        const UPSERT_SHARED: &str = "title = COALESCE(excluded.title, sessions.title),
                title_state = CASE
                    WHEN excluded.title IS NOT NULL THEN excluded.title_state
                    ELSE sessions.title_state
                END,
                model = COALESCE(excluded.model, sessions.model),
                thinking = COALESCE(excluded.thinking, sessions.thinking),
                cwd = excluded.cwd,
                additional_directories = excluded.additional_directories,
                updated_at = excluded.updated_at,
                parent_session_id = COALESCE(excluded.parent_session_id, sessions.parent_session_id),
                fork_from_id = COALESCE(excluded.fork_from_id, sessions.fork_from_id),
                fork_at_turn_id = COALESCE(excluded.fork_at_turn_id, sessions.fork_at_turn_id),
                agent_path = COALESCE(excluded.agent_path, sessions.agent_path),
                rollout_path = COALESCE(excluded.rollout_path, sessions.rollout_path)";
        let update_clause = match source {
            SessionUpsertSource::RuntimeLive => {
                format!("{UPSERT_SHARED}, last_activity_at = excluded.last_activity_at")
            }
            SessionUpsertSource::RolloutIndex => {
                format!(
                    "{UPSERT_SHARED}, created_at = excluded.created_at, last_activity_at = MAX(excluded.last_activity_at, sessions.last_activity_at)"
                )
            }
        };
        let sql = format!(
            "INSERT INTO sessions (id, title, title_state, model, thinking, cwd, additional_directories, ephemeral, created_at, updated_at, last_activity_at, rollout_path, parent_session_id, fork_from_id, fork_at_turn_id, agent_path, schema_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 3)
             ON CONFLICT(id) DO UPDATE SET {update_clause}"
        );
        conn.execute(
            &sql,
            params![
                meta.session_id.as_str(),
                meta.title,
                title_state_str,
                meta.model,
                meta.reasoning_effort_selection,
                meta.cwd.to_string_lossy().to_string(),
                additional_directories,
                meta.ephemeral as i32,
                meta.created_at.timestamp(),
                meta.updated_at.timestamp(),
                meta.last_activity_at.timestamp(),
                rollout_path_str,
                parent_session_id,
                fork_from_id,
                fork_at_turn_id,
                agent_path,
            ],
        )
        .context("failed to upsert session")?;
        Ok(())
    }

    /// Returns true when durable sessions need rollout metadata backfilled into SQLite.
    pub fn session_index_backfill_required(&self) -> Result<bool> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions
                 WHERE ephemeral = 0 AND (rollout_path IS NULL OR rollout_path = '')",
                [],
                |row| row.get(0),
            )
            .context("failed to check session index backfill requirement")?;
        Ok(count > 0)
    }

    /// Retrieves a session's metadata by Native session id.
    pub fn get_session(&self, id: &SessionId) -> Result<Option<SessionIndexRow>> {
        Ok(self.get_session_index(id)?.map(|record| record.session))
    }

    /// Returns resume/list index fields for a Native session id.
    pub fn get_session_index(&self, id: &SessionId) -> Result<Option<SessionIndexRecord>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, title, title_state, model, thinking, cwd, additional_directories, ephemeral, created_at, updated_at, last_activity_at, parent_session_id, fork_from_id, fork_at_turn_id, agent_path, rollout_path
                 FROM sessions WHERE id = ?1",
            )
            .context("failed to prepare get_session_index statement")?;
        let result = stmt.query_row(params![id.as_str()], |row| {
            let session = parse_session_index_row(row)?;
            let rollout_path = row
                .get::<_, Option<String>>(15)?
                .map(PathBuf::from)
                .filter(|path| !path.as_os_str().is_empty());
            Ok(SessionIndexRecord {
                session,
                rollout_path,
            })
        });

        match result {
            Ok(index) => Ok(Some(index)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Lists durable user-visible sessions (roots and forks, excluding subagents).
    pub fn list_root_sessions(&self) -> Result<Vec<SessionIndexRow>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, title, title_state, model, thinking, cwd, additional_directories, ephemeral, created_at, updated_at, last_activity_at, parent_session_id, fork_from_id, fork_at_turn_id, agent_path
                 FROM sessions
                 WHERE ephemeral = 0 AND agent_path IS NULL
                 ORDER BY last_activity_at DESC, updated_at DESC",
            )
            .context("failed to prepare list_root_sessions statement")?;
        collect_session_index_rows(&mut stmt, "root sessions")
    }

    /// Lists all sessions ordered by most recently updated.
    pub fn list_sessions(&self) -> Result<Vec<SessionIndexRow>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, title, title_state, model, thinking, cwd, additional_directories, ephemeral, created_at, updated_at, last_activity_at, parent_session_id, fork_from_id, fork_at_turn_id, agent_path
                 FROM sessions ORDER BY last_activity_at DESC, updated_at DESC",
            )
            .context("failed to prepare list_sessions statement")?;
        collect_session_index_rows(&mut stmt, "sessions")
    }

    /// Bulk map of session id → on-disk rollout path for list enrichment.
    pub fn list_rollout_paths(&self) -> Result<std::collections::HashMap<SessionId, PathBuf>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, rollout_path FROM sessions
                 WHERE rollout_path IS NOT NULL AND rollout_path != ''",
            )
            .context("failed to prepare list_rollout_paths statement")?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let path: String = row.get(1)?;
                Ok((SessionId::from_string(id), PathBuf::from(path)))
            })
            .context("failed to query list_rollout_paths")?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (id, path) = row.context("failed to read list_rollout_paths row")?;
            out.insert(id, path);
        }
        Ok(out)
    }

    /// Deletes a session and its related data.
    pub fn delete_session(&self, id: &SessionId) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id.as_str()])
            .context("failed to delete session")?;
        Ok(())
    }

    // === Session Stats ===

    /// Inserts or updates session token statistics.
    pub fn update_stats(&self, id: &SessionId, stats: &SessionStats) -> Result<()> {
        let occupancy_json = stats
            .last_context_occupancy
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("failed to serialize last_context_occupancy")?;
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute(
            "INSERT INTO session_stats (session_id, total_input_tokens, total_output_tokens,
                total_tokens, total_cache_creation_tokens, total_cache_read_tokens, last_input_tokens,
                turn_count, prompt_token_estimate, last_context_occupancy)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(session_id) DO UPDATE SET
                total_input_tokens = excluded.total_input_tokens,
                total_output_tokens = excluded.total_output_tokens,
                total_tokens = excluded.total_tokens,
                total_cache_creation_tokens = excluded.total_cache_creation_tokens,
                total_cache_read_tokens = excluded.total_cache_read_tokens,
                last_input_tokens = excluded.last_input_tokens,
                turn_count = excluded.turn_count,
                prompt_token_estimate = excluded.prompt_token_estimate,
                last_context_occupancy = excluded.last_context_occupancy",
            params![
                id.as_str(),
                stats.total_input_tokens as i64,
                stats.total_output_tokens as i64,
                stats.total_tokens as i64,
                stats.total_cache_creation_tokens as i64,
                stats.total_cache_read_tokens as i64,
                stats.last_input_tokens as i64,
                stats.turn_count as i64,
                stats.prompt_token_estimate as i64,
                occupancy_json,
            ],
        )
        .context("failed to update session stats")?;
        Ok(())
    }

    /// Retrieves session token statistics.
    pub fn get_stats(&self, id: &SessionId) -> Result<Option<SessionStats>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let result = conn.query_row(
            "SELECT total_input_tokens, total_output_tokens, total_tokens, total_cache_creation_tokens,
                    total_cache_read_tokens, last_input_tokens, turn_count, prompt_token_estimate,
                    last_context_occupancy
             FROM session_stats WHERE session_id = ?1",
            params![id.as_str()],
            |row| {
                let occupancy_json: Option<String> = row.get(8)?;
                let last_context_occupancy = occupancy_json.and_then(|json| {
                    serde_json::from_str::<ContextOccupancy>(&json).ok()
                });
                Ok(SessionStats {
                    total_input_tokens: row.get::<_, i64>(0)? as usize,
                    total_output_tokens: row.get::<_, i64>(1)? as usize,
                    total_tokens: row.get::<_, i64>(2)? as usize,
                    total_cache_creation_tokens: row.get::<_, i64>(3)? as usize,
                    total_cache_read_tokens: row.get::<_, i64>(4)? as usize,
                    last_input_tokens: row.get::<_, i64>(5)? as usize,
                    turn_count: row.get::<_, i64>(6)? as usize,
                    prompt_token_estimate: row.get::<_, i64>(7)? as usize,
                    last_context_occupancy,
                })
            },
        );

        match result {
            Ok(stats) => Ok(Some(stats)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Bulk turn_count lookup for session/list message_count enrichment.
    pub fn list_turn_counts(
        &self,
        session_ids: &[SessionId],
    ) -> Result<std::collections::HashMap<SessionId, u32>> {
        if session_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut out = std::collections::HashMap::with_capacity(session_ids.len());
        let mut stmt = conn
            .prepare("SELECT turn_count FROM session_stats WHERE session_id = ?1")
            .context("failed to prepare list_turn_counts statement")?;
        for session_id in session_ids {
            match stmt.query_row(params![session_id.as_str()], |row| row.get::<_, i64>(0)) {
                Ok(turn_count) => {
                    out.insert(*session_id, turn_count as u32);
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(out)
    }

    // === Pending Messages ===

    /// Pushes a pending message to the specified queue.
    pub fn push_pending(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        item: &PendingInputItem,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let (kind_str, content) = pending_kind_parts(&item.kind);
        let metadata_str = item.metadata.as_ref().map(|v| v.to_string());
        conn.execute(
            "INSERT INTO pending_messages (session_id, queue_type, kind, content, pending_input_id, metadata, created_at, position)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                (SELECT COALESCE(MAX(position), 0) + 1 FROM pending_messages WHERE session_id = ?1 AND queue_type = ?2)",
            params![
                session_id.as_str(),
                queue.as_str(),
                kind_str,
                content,
                item.id.to_string(),
                metadata_str,
                item.created_at.timestamp(),
            ],
        )
        .context("failed to push pending message")?;
        Ok(())
    }

    /// Replaces one pending message's content (kind/content/metadata), keyed
    /// by its stable `pending_input_id` (`session/queue/update`). Returns
    /// whether the entry still existed.
    pub fn update_pending_content(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        item: &PendingInputItem,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let (kind_str, content) = pending_kind_parts(&item.kind);
        let metadata_str = item.metadata.as_ref().map(|v| v.to_string());
        let changes = conn
            .execute(
                "UPDATE pending_messages SET kind = ?4, content = ?5, metadata = ?6
                 WHERE session_id = ?1 AND queue_type = ?2 AND pending_input_id = ?3",
                params![
                    session_id.as_str(),
                    queue.as_str(),
                    item.id.to_string(),
                    kind_str,
                    content,
                    metadata_str,
                ],
            )
            .context("failed to update pending message")?;
        Ok(changes == 1)
    }

    /// Merges one key into a pending message's metadata JSON (used for the
    /// `clientUserMessageId` dedup key, 01 §4.3). Returns whether the entry
    /// existed.
    pub fn set_pending_metadata_field(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        pending_input_id: &QueueItemId,
        key: &str,
        value: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let changes = conn
            .execute(
                "UPDATE pending_messages
                 SET metadata = json_set(COALESCE(metadata, '{}'), '$.' || ?4, ?5)
                 WHERE session_id = ?1 AND queue_type = ?2 AND pending_input_id = ?3",
                params![
                    session_id.as_str(),
                    queue.as_str(),
                    pending_input_id.to_string(),
                    key,
                    value,
                ],
            )
            .context("failed to update pending message metadata")?;
        Ok(changes == 1)
    }

    /// Rewrites queue positions to 1..=N following `ordered_ids`
    /// (`session/queue/update` reorder). Ids not listed keep their relative
    /// order at the end.
    pub fn set_pending_positions(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        ordered_ids: &[QueueItemId],
    ) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        for (index, id) in ordered_ids.iter().enumerate() {
            conn.execute(
                "UPDATE pending_messages SET position = ?4
                 WHERE session_id = ?1 AND queue_type = ?2 AND pending_input_id = ?3",
                params![
                    session_id.as_str(),
                    queue.as_str(),
                    id.to_string(),
                    (index + 1) as i64,
                ],
            )
            .context("failed to reorder pending messages")?;
        }
        Ok(())
    }

    /// Lists pending messages of one queue without draining them
    /// (subscription snapshots, 08 §4).
    pub fn list_pending(
        &self,
        session_id: &SessionId,
        queue: QueueType,
    ) -> Result<Vec<PendingInputItem>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT kind, content, pending_input_id, metadata, created_at
                 FROM pending_messages
                 WHERE session_id = ?1 AND queue_type = ?2
                 ORDER BY position ASC, id ASC",
            )
            .context("failed to prepare list_pending statement")?;
        collect_pending_rows(&mut stmt, session_id, queue)
    }

    /// Drains all pending messages from the specified queue, deleting them in the process.
    pub fn drain_pending(
        &self,
        session_id: &SessionId,
        queue: QueueType,
    ) -> Result<Vec<PendingInputItem>> {
        let mut conn = self.conn.lock().expect("database mutex poisoned");

        let tx = conn
            .transaction()
            .context("failed to begin drain transaction")?;

        let items = {
            let mut stmt = tx
                .prepare(
                    "SELECT kind, content, pending_input_id, metadata, created_at
                     FROM pending_messages
                     WHERE session_id = ?1 AND queue_type = ?2
                     ORDER BY position ASC, id ASC",
                )
                .context("failed to prepare drain_pending statement")?;
            collect_pending_rows(&mut stmt, session_id, queue)?
        };

        tx.execute(
            "DELETE FROM pending_messages WHERE session_id = ?1 AND queue_type = ?2",
            params![session_id.as_str(), queue.as_str()],
        )
        .context("failed to delete drained messages")?;

        tx.commit().context("failed to commit drain transaction")?;

        Ok(items)
    }

    /// Removes one pending message from the specified queue by its stable pending input id.
    pub fn remove_pending_by_id(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        pending_input_id: &QueueItemId,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let affected = conn
            .execute(
                "DELETE FROM pending_messages
                 WHERE session_id = ?1 AND queue_type = ?2 AND pending_input_id = ?3",
                params![
                    session_id.as_str(),
                    queue.as_str(),
                    pending_input_id.to_string(),
                ],
            )
            .context("failed to remove pending message by id")?;
        Ok(affected > 0)
    }

    /// Clears all pending messages from the specified queue.
    pub fn clear_pending(&self, session_id: &SessionId, queue: QueueType) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute(
            "DELETE FROM pending_messages WHERE session_id = ?1 AND queue_type = ?2",
            params![session_id.as_str(), queue.as_str()],
        )
        .context("failed to clear pending messages")?;
        Ok(())
    }

    /// Counts pending messages in the specified queue.
    #[allow(dead_code)]
    pub fn count_pending(&self, session_id: &SessionId, queue: QueueType) -> Result<usize> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pending_messages WHERE session_id = ?1 AND queue_type = ?2",
            params![session_id.as_str(), queue.as_str()],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

fn collect_pending_rows(
    stmt: &mut rusqlite::Statement<'_>,
    session_id: &SessionId,
    queue: QueueType,
) -> Result<Vec<PendingInputItem>> {
    stmt.query_map(params![session_id.as_str(), queue.as_str()], |row| {
        Ok(pending_input_from_row(
            &row.get::<_, String>(0)?,
            &row.get::<_, String>(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })
    .context("failed to query pending messages")?
    .collect::<std::result::Result<Vec<_>, _>>()
    .context("failed to decode pending messages")
}

fn collect_session_index_rows(
    stmt: &mut rusqlite::Statement<'_>,
    label: &str,
) -> Result<Vec<SessionIndexRow>> {
    stmt.query_map([], parse_session_index_row)
        .with_context(|| format!("failed to query {label}"))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to parse {label}"))
}

fn ensure_column(conn: &Connection, table: &str, column: &str, alter_sql: &str) -> Result<bool> {
    if table_has_column(conn, table, column)? {
        return Ok(false);
    }
    conn.execute(alter_sql, [])
        .with_context(|| format!("failed to add {table}.{column} column"))?;
    Ok(true)
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("failed to inspect {table} schema"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to read sessions schema")?;
    for column_name in columns {
        if column_name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn parse_session_index_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionIndexRow> {
    let id_str: String = row.get(0)?;
    let title: Option<String> = row.get(1)?;
    let title_state_str: String = row.get(2)?;
    let model: Option<String> = row.get(3)?;
    let thinking: Option<String> = row.get(4)?;
    let cwd_str: String = row.get(5)?;
    let additional_directories_str: String = row.get(6)?;
    let ephemeral: i32 = row.get(7)?;
    let created_at: i64 = row.get(8)?;
    let updated_at: i64 = row.get(9)?;
    let last_activity_at: i64 = row.get(10)?;
    let parent_session_id = row
        .get::<_, Option<String>>(11)?
        .map(|value| parse_session_id_column(value, 11))
        .transpose()?;
    let fork_from_id = row
        .get::<_, Option<String>>(12)?
        .map(|value| parse_session_id_column(value, 12))
        .transpose()?;
    let fork_at_turn_id = row
        .get::<_, Option<String>>(13)?
        .map(|value| parse_turn_id_column(value, 13))
        .transpose()?;
    let agent_path = match row.get::<_, Option<String>>(14)? {
        Some(path) if path.is_empty() => Some("subagent".to_string()),
        other => other,
    };

    let title_state = match title_state_str.as_str() {
        "generating" => SessionTitleState::Generating,
        "final" => SessionTitleState::Final(devo_protocol::SessionTitleFinalSource::ModelGenerated),
        // Legacy persisted rows used "provisional" for in-flight auto titles.
        "provisional" => SessionTitleState::Unset,
        _ => SessionTitleState::Unset,
    };

    Ok(SessionIndexRow {
        session_id: parse_session_id_column(id_str, 0)?,
        cwd: PathBuf::from(&cwd_str),
        additional_directories: parse_additional_directories_column(additional_directories_str, 6)?,
        created_at: Utc
            .timestamp_opt(created_at, 0)
            .single()
            .unwrap_or_else(Utc::now),
        updated_at: Utc
            .timestamp_opt(updated_at, 0)
            .single()
            .unwrap_or_else(Utc::now),
        last_activity_at: Utc
            .timestamp_opt(last_activity_at, 0)
            .single()
            .unwrap_or_else(Utc::now),
        title,
        title_state,
        parent_session_id,
        fork_from_id,
        fork_at_turn_id,
        agent_path,
        ephemeral: ephemeral != 0,
        model,
        reasoning_effort_selection: thinking,
    })
}

fn parse_session_id_column(id: String, column: usize) -> rusqlite::Result<SessionId> {
    if id.is_empty() {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session id must be non-empty",
            )),
        ));
    }
    Ok(SessionId::from_string(id))
}

fn parse_turn_id_column(id: String, column: usize) -> rusqlite::Result<TurnId> {
    if id.is_empty() {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "turn id must be non-empty",
            )),
        ));
    }
    Ok(TurnId::from_string(id))
}

fn parse_additional_directories_column(
    value: String,
    column: usize,
) -> rusqlite::Result<Vec<PathBuf>> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
    })
}

/// Maps a `PendingInputKind` to its `(kind, content)` storage pair (shared
/// by `push_pending` and `update_pending_content`).
fn pending_kind_parts(kind: &PendingInputKind) -> (&'static str, String) {
    match kind {
        PendingInputKind::UserText { text } => ("user_text", text.clone()),
        PendingInputKind::UserInput {
            input,
            display_text,
            prompt_text,
            prompt_messages,
            prompt_images,
        } => {
            let content = serde_json::json!({
                "input": input,
                "display_text": display_text,
                "prompt_text": prompt_text,
                "prompt_messages": prompt_messages,
                "prompt_images": prompt_images,
            });
            ("user_input", content.to_string())
        }
        PendingInputKind::ToolCallBlockedByHook {
            tool_use_id,
            reason,
        } => {
            let content = serde_json::json!({
                "tool_use_id": tool_use_id,
                "reason": reason,
            });
            ("tool_call_blocked", content.to_string())
        }
        PendingInputKind::BudgetLimitSteering => ("budget_limit", String::new()),
    }
}

/// Maps one `pending_messages` row to its `PendingInputItem` (shared by
/// `drain_pending` and `list_pending`).
fn pending_input_from_row(
    kind_str: &str,
    content: &str,
    pending_input_id: Option<String>,
    metadata_str: Option<String>,
    created_at: i64,
) -> PendingInputItem {
    let kind = match kind_str {
        "user_text" => PendingInputKind::UserText {
            text: content.to_string(),
        },
        "user_input" => serde_json::from_str::<serde_json::Value>(content)
            .ok()
            .and_then(|value| {
                Some(PendingInputKind::UserInput {
                    input: serde_json::from_value(value.get("input")?.clone()).ok()?,
                    display_text: value
                        .get("display_text")?
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    prompt_text: value
                        .get("prompt_text")?
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    prompt_messages: value
                        .get("prompt_messages")
                        .and_then(|messages| serde_json::from_value(messages.clone()).ok())
                        .unwrap_or_default(),
                    prompt_images: value
                        .get("prompt_images")
                        .and_then(|images| serde_json::from_value(images.clone()).ok())
                        .unwrap_or_default(),
                })
            })
            .unwrap_or(PendingInputKind::UserText {
                text: content.to_string(),
            }),
        "tool_call_blocked" => {
            let parsed: serde_json::Value = serde_json::from_str(content).unwrap_or_default();
            PendingInputKind::ToolCallBlockedByHook {
                tool_use_id: parsed["tool_use_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                reason: parsed["reason"].as_str().unwrap_or_default().to_string(),
            }
        }
        "budget_limit" => PendingInputKind::BudgetLimitSteering,
        _ => PendingInputKind::UserText {
            text: content.to_string(),
        },
    };
    PendingInputItem {
        id: pending_input_id
            .map(QueueItemId::from_string)
            .unwrap_or_default(),
        kind,
        metadata: metadata_str.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: Utc
            .timestamp_opt(created_at, 0)
            .single()
            .unwrap_or_else(Utc::now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().expect("create temp dir");
        let db_path = dir.path().join("test.db");
        let db = Database::open(db_path).expect("open database");
        (db, dir)
    }

    #[test]
    fn schema_meta_records_current_schema_version() {
        let (db, _dir) = test_db();
        assert_eq!(
            db.schema_version().expect("read schema version"),
            Some(CURRENT_SCHEMA_VERSION)
        );
        // Re-opening an existing database keeps the recorded version.
        let (db, dir) = test_db();
        drop(db);
        let db = Database::open(dir.path().join("test.db")).expect("reopen database");
        assert_eq!(
            db.schema_version().expect("read schema version"),
            Some(CURRENT_SCHEMA_VERSION)
        );
    }

    #[test]
    fn migration_renames_legacy_btw_queue_rows_to_steer() {
        let dir = TempDir::new().expect("create temp dir");
        let db_path = dir.path().join("legacy.db");
        let session_id = SessionId::new();
        {
            let conn = Connection::open(&db_path).expect("open legacy database");
            conn.execute_batch(
                "
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    cwd TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE pending_messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    queue_type TEXT NOT NULL CHECK(queue_type IN ('turn', 'btw')),
                    kind TEXT NOT NULL,
                    content TEXT NOT NULL,
                    pending_input_id TEXT,
                    metadata TEXT,
                    created_at INTEGER NOT NULL,
                    position INTEGER
                );
                CREATE INDEX idx_pending_session
                    ON pending_messages(session_id, queue_type);
                ",
            )
            .expect("create legacy queue tables");
            conn.execute(
                "INSERT INTO sessions (id, cwd, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![session_id.to_string(), ".", 0, 0],
            )
            .expect("insert legacy session");
            conn.execute(
                "INSERT INTO pending_messages
                    (session_id, queue_type, kind, content, pending_input_id, created_at, position)
                 VALUES (?1, 'btw', 'user_text', 'keep steering', ?2, 0, 1)",
                params![session_id.to_string(), QueueItemId::new().to_string()],
            )
            .expect("insert legacy steer row");
        }

        let db = Database::open(db_path).expect("migrate legacy database");
        assert_eq!(
            db.count_pending(&session_id, QueueType::Steer)
                .expect("count migrated steer row"),
            1
        );
        let conn = db.conn.lock().expect("database mutex poisoned");
        let queue_type: String = conn
            .query_row("SELECT queue_type FROM pending_messages", [], |row| {
                row.get(0)
            })
            .expect("read migrated queue type");
        assert_eq!(queue_type, "steer");
        let old_value = conn.execute(
            "INSERT INTO pending_messages
                (session_id, queue_type, kind, content, created_at, position)
             VALUES (?1, 'btw', 'user_text', 'obsolete', 0, 2)",
            params![session_id.to_string()],
        );
        assert!(old_value.is_err(), "legacy queue type must be rejected");
    }

    #[test]
    fn migration_backfills_legacy_session_stats_total_tokens() {
        let dir = TempDir::new().expect("create temp dir");
        let db_path = dir.path().join("legacy.db");
        let session_id = SessionId::new();
        {
            let conn = Connection::open(&db_path).expect("open legacy database");
            conn.execute_batch(
                "
                CREATE TABLE session_stats (
                    session_id TEXT PRIMARY KEY,
                    total_input_tokens INTEGER NOT NULL DEFAULT 0,
                    total_output_tokens INTEGER NOT NULL DEFAULT 0,
                    total_cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                    total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    last_input_tokens INTEGER NOT NULL DEFAULT 0,
                    turn_count INTEGER NOT NULL DEFAULT 0,
                    prompt_token_estimate INTEGER NOT NULL DEFAULT 0
                );
                ",
            )
            .expect("create legacy session_stats");
            conn.execute(
                "INSERT INTO session_stats (session_id, total_input_tokens, total_output_tokens)
                 VALUES (?1, ?2, ?3)",
                params![session_id.to_string(), 40_i64, 2_i64],
            )
            .expect("insert legacy stats");
        }

        let db = Database::open(db_path).expect("migrate database");
        let stats = db
            .get_stats(&session_id)
            .expect("get migrated stats")
            .expect("stats row exists");

        assert_eq!(stats.total_tokens, 42);
    }

    fn sample_session(id: &str) -> SessionIndexRow {
        SessionIndexRow {
            session_id: SessionId::from_string(id.to_owned()),
            cwd: PathBuf::from("/tmp"),
            additional_directories: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_activity_at: Utc::now(),
            title: Some("Test Session".into()),
            title_state: SessionTitleState::Generating,
            parent_session_id: None,
            fork_from_id: None,
            fork_at_turn_id: None,
            agent_path: None,
            ephemeral: false,
            model: Some("claude-sonnet-4-20250514".into()),
            reasoning_effort_selection: None,
        }
    }

    #[test]
    fn session_index_crud_and_ordering() {
        let (db, _dir) = test_db();
        let mut meta = sample_session("session-1");
        meta.additional_directories = vec![PathBuf::from("/tmp/shared")];
        db.upsert_session(&meta, None).expect("upsert");
        let retrieved = db.get_session(&meta.session_id).expect("get").expect("row");
        assert_eq!(retrieved.session_id, meta.session_id);
        assert_eq!(
            retrieved.additional_directories,
            meta.additional_directories
        );

        let mut newer = sample_session("session-2");
        let baseline = Utc::now();
        meta.updated_at = baseline;
        meta.last_activity_at = baseline + chrono::Duration::seconds(10);
        newer.updated_at = baseline;
        newer.last_activity_at = baseline;
        db.upsert_session(&meta, None).expect("upsert meta");
        db.upsert_session(&newer, None).expect("upsert newer");
        let sessions = db.list_sessions().expect("list");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, meta.session_id);

        db.delete_session(&meta.session_id).expect("delete");
        assert!(db.get_session(&meta.session_id).expect("get").is_none());
    }

    #[test]
    fn list_sessions_rejects_empty_persisted_session_id() {
        let (db, _dir) = test_db();
        let conn = db.conn.lock().expect("database mutex poisoned");
        conn.execute(
            "INSERT INTO sessions (id, title, title_state, cwd, ephemeral, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "",
                "Corrupt Session",
                "provisional",
                "/tmp",
                0_i32,
                1_i64,
                1_i64
            ],
        )
        .expect("insert corrupt session");
        drop(conn);

        let error = db
            .list_sessions()
            .expect_err("empty persisted session id should fail closed");
        let message = error
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            message.contains("session id must be non-empty"),
            "{message}"
        );
    }

    #[test]
    fn session_stats_roundtrip() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");
        db.upsert_session(&meta, None).expect("upsert");
        let occupancy = ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 5_000,
            /*tools_builtin*/ 20_000, /*tools_mcp*/ 15_000, /*conversation*/ 50_000,
        );
        let stats = SessionStats {
            total_input_tokens: 1000,
            total_output_tokens: 500,
            total_tokens: 1500,
            total_cache_creation_tokens: 100,
            total_cache_read_tokens: 50,
            last_input_tokens: 200,
            turn_count: 5,
            prompt_token_estimate: 800,
            last_context_occupancy: Some(occupancy.clone()),
        };
        db.update_stats(&meta.session_id, &stats).expect("update");
        let retrieved = db.get_stats(&meta.session_id).expect("get").expect("row");
        assert_eq!(retrieved.total_input_tokens, 1000);
        assert_eq!(retrieved.turn_count, 5);
        assert_eq!(retrieved.last_context_occupancy, Some(occupancy));
    }

    #[test]
    fn pending_queue_operations() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");
        db.upsert_session(&meta, None).expect("upsert");
        assert!(
            db.drain_pending(&meta.session_id, QueueType::Turn)
                .expect("drain empty")
                .is_empty()
        );

        let turn_item = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "turn".into(),
            },
            None,
            Utc::now(),
        );
        let steer_item = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "steer".into(),
            },
            None,
            Utc::now(),
        );
        let second = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "second".into(),
            },
            Some(serde_json::json!({"clientUserMessageId":"stable-message-1"})),
            Utc::now(),
        );
        db.push_pending(&meta.session_id, QueueType::Turn, &turn_item)
            .expect("push turn");
        db.push_pending(&meta.session_id, QueueType::Steer, &steer_item)
            .expect("push steer");
        db.push_pending(&meta.session_id, QueueType::Turn, &second)
            .expect("push second");
        assert_eq!(
            db.count_pending(&meta.session_id, QueueType::Turn)
                .expect("count turn"),
            2
        );
        assert_eq!(
            db.count_pending(&meta.session_id, QueueType::Steer)
                .expect("count steer"),
            1
        );
        assert!(
            db.remove_pending_by_id(&meta.session_id, QueueType::Turn, &turn_item.id)
                .expect("remove first")
        );
        db.clear_pending(&meta.session_id, QueueType::Steer)
            .expect("clear steer");
        let remaining = db
            .drain_pending(&meta.session_id, QueueType::Turn)
            .expect("drain");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, second.id);
        assert_eq!(
            remaining[0].metadata,
            Some(serde_json::json!({"clientUserMessageId":"stable-message-1"}))
        );
    }

    #[test]
    fn session_index_backfill_required_detects_missing_rollout_path() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");

        assert!(
            !db.session_index_backfill_required()
                .expect("check empty db")
        );
        db.upsert_session(&meta, None)
            .expect("upsert without rollout path");

        assert!(
            db.session_index_backfill_required()
                .expect("check missing rollout path")
        );
        db.upsert_rollout_index_session(&meta, Some("/tmp/session.jsonl".as_ref()))
            .expect("upsert rollout path");
        assert!(
            !db.session_index_backfill_required()
                .expect("check populated rollout path")
        );
    }

    #[test]
    fn list_root_sessions_excludes_subagents_and_ephemeral() {
        let (db, _dir) = test_db();
        let root_id = SessionId::new();
        let subagent_id = SessionId::new();
        let ephemeral_id = SessionId::new();
        let rollout_path = PathBuf::from("/tmp/root.jsonl");

        let mut root = sample_session(root_id.as_ref());
        root.session_id = root_id;
        db.upsert_session(&root, Some(rollout_path.as_path()))
            .expect("upsert root");

        let mut subagent = sample_session(subagent_id.as_ref());
        subagent.session_id = subagent_id;
        subagent.parent_session_id = Some(root_id);
        subagent.agent_path = Some("root/review".into());
        db.upsert_session(&subagent, Some("/tmp/subagent.jsonl".as_ref()))
            .expect("upsert subagent");

        let mut ephemeral = sample_session(ephemeral_id.as_ref());
        ephemeral.session_id = ephemeral_id;
        ephemeral.ephemeral = true;
        db.upsert_session(&ephemeral, None)
            .expect("upsert ephemeral");

        let roots = db.list_root_sessions().expect("list root sessions");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].session_id, root_id);

        let index = db
            .get_session_index(&root_id)
            .expect("get index")
            .expect("root index");
        assert_eq!(index.rollout_path, Some(rollout_path));
        assert_eq!(index.session.parent_session_id, None);

        let subagent_index = db
            .get_session_index(&subagent_id)
            .expect("get subagent index")
            .expect("subagent index");
        assert_eq!(subagent_index.session.parent_session_id, Some(root_id));
    }
}
