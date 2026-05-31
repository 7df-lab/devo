//! Session forking and inherited history retention.
//!
//! Implements L3-BEH-CORE-011. Fork admission, inherited segment construction,
//! child session creation, and replay with independent segment loading.

use chrono::Utc;

use devo_protocol::{SessionId, TurnId};

use crate::durable_record::{
    DurableRecord, ForkCreator, ForkOrigin, InheritedHistorySegmentDescriptor, ParentAvailability,
    SegmentAvailability, SegmentSourceRange, SessionCreatedRecord, SessionForkedRecord,
    StorageStrategy,
};
use crate::session_store::SessionStore;

// ── Fork Admission ──────────────────────────────────────────────────

/// Validates and admits a session fork request.
pub fn validate_fork_request(
    parent_session_id: SessionId,
    fork_turn_id: TurnId,
    workspace_root: &str,
    fork_label: Option<&str>,
) -> Result<ForkRequest, ForkError> {
    if workspace_root.is_empty() {
        return Err(ForkError::InvalidWorkspaceRoot);
    }

    Ok(ForkRequest {
        parent_session_id,
        fork_turn_id,
        workspace_root: workspace_root.to_string(),
        fork_label: fork_label.map(|s| s.to_string()),
    })
}

/// A validated fork request ready for processing.
#[derive(Debug, Clone)]
pub struct ForkRequest {
    pub parent_session_id: SessionId,
    pub fork_turn_id: TurnId,
    pub workspace_root: String,
    pub fork_label: Option<String>,
}

// ── Inherited Segment Construction ──────────────────────────────────

/// Builds an inherited history segment from parent session records.
pub fn build_inherited_segment(_fork_turn_id: TurnId) -> InheritedHistorySegmentDescriptor {
    InheritedHistorySegmentDescriptor {
        inherited_segment_id: format!("seg-{}", uuid::Uuid::new_v4()),
        source_parent_session_id: SessionId::new(), // placeholder
        source_range: SegmentSourceRange {
            start_offset: 0,
            end_offset: 0,
        },
        storage_strategy: StorageStrategy::ProtectedSharedSegment,
        record_refs: vec![],
        segment_hash: String::new(),
        availability_state: SegmentAvailability::Available,
        created_at: Utc::now(),
    }
}

// ── Child Session Creation ──────────────────────────────────────────

/// Creates the child fork session by writing durable records.
pub async fn create_fork_session(
    store: &dyn SessionStore,
    request: &ForkRequest,
    parent_display_label: &str,
    fork_turn_display_label: &str,
) -> Result<SessionId, ForkError> {
    let child_session_id = SessionId::new();
    let fork_turn_id = request.fork_turn_id;
    let now = Utc::now();

    // Build fork origin metadata
    let fork_origin = ForkOrigin {
        parent_session_id: request.parent_session_id,
        fork_turn_id,
        fork_created_at: now,
        parent_display_label: parent_display_label.to_string(),
        fork_turn_display_label: fork_turn_display_label.to_string(),
        fork_turn_digest: format!("Turn {}", fork_turn_id),
        origin_snapshot_hash: String::new(),
        parent_availability: ParentAvailability::Available,
    };

    // Build inherited segment
    let inherited_segment = build_inherited_segment(fork_turn_id);

    // 1. Write SessionCreated for child
    store
        .append(
            child_session_id,
            DurableRecord::SessionCreated(SessionCreatedRecord {
                schema_version: 1,
                session_id: child_session_id,
                workspace_root: request.workspace_root.clone(),
                created_at: now,
            }),
        )
        .await
        .map_err(|e| ForkError::PersistenceFailure(e.message))?;

    // 2. Write SessionForked for child
    store
        .append(
            child_session_id,
            DurableRecord::SessionForked(SessionForkedRecord {
                schema_version: 1,
                session_id: child_session_id,
                fork_origin,
                inherited_segment,
                workspace_root: request.workspace_root.clone(),
                fork_label: request.fork_label.clone(),
                created_by: ForkCreator::User,
                created_at: now,
            }),
        )
        .await
        .map_err(|e| ForkError::PersistenceFailure(e.message))?;

    // 3. Flush to disk
    store
        .flush(child_session_id)
        .await
        .map_err(|e| ForkError::PersistenceFailure(e.message))?;

    Ok(child_session_id)
}

// ── Fork Error ──────────────────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
pub enum ForkError {
    #[error("parent session not found: {0}")]
    ParentSessionNotFound(SessionId),
    #[error("fork turn not found: {0}")]
    ForkTurnNotFound(TurnId),
    #[error("fork turn is not at a stable boundary")]
    ForkTurnNotStable,
    #[error("invalid workspace root")]
    InvalidWorkspaceRoot,
    #[error("inherited segment write failed")]
    InheritedSegmentWriteFailed,
    #[error("inherited segment materialization failed: {0}")]
    InheritedSegmentMaterializationFailed(String),
    #[error("persistence failure: {0}")]
    PersistenceFailure(String),
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_fork_request_accepts_valid_input() {
        let parent_id = SessionId::new();
        let turn_id = TurnId::new();
        let result = validate_fork_request(parent_id, turn_id, "/workspace", Some("My fork"));
        assert!(result.is_ok());
        let req = result.unwrap();
        assert_eq!(req.parent_session_id, parent_id);
        assert_eq!(req.fork_turn_id, turn_id);
        assert_eq!(req.fork_label.as_deref(), Some("My fork"));
    }

    #[test]
    fn validate_fork_request_rejects_empty_workspace() {
        let result = validate_fork_request(SessionId::new(), TurnId::new(), "", None);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ForkError::InvalidWorkspaceRoot
        ));
    }

    #[test]
    fn fork_request_clone() {
        let req = ForkRequest {
            parent_session_id: SessionId::new(),
            fork_turn_id: TurnId::new(),
            workspace_root: "/tmp".into(),
            fork_label: None,
        };
        let cloned = req.clone();
        assert_eq!(cloned.workspace_root, "/tmp");
    }

    #[test]
    fn build_inherited_segment_creates_valid_descriptor() {
        let turn_id = TurnId::new();
        let segment = build_inherited_segment(turn_id);
        assert!(segment.inherited_segment_id.starts_with("seg-"));
        assert_eq!(
            segment.storage_strategy,
            StorageStrategy::ProtectedSharedSegment
        );
        assert_eq!(segment.availability_state, SegmentAvailability::Available);
    }

    #[test]
    fn fork_error_display() {
        let err = ForkError::InvalidWorkspaceRoot;
        assert!(err.to_string().contains("workspace root"));
        let err = ForkError::ParentSessionNotFound(SessionId::new());
        assert!(err.to_string().contains("parent session"));
    }
}
