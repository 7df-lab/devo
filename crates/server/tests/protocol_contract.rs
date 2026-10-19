use std::collections::VecDeque;

use chrono::Utc;
use devo_core::{ItemId, SessionId, SessionTitleFinalSource, SessionTitleState, TurnId};
use devo_protocol::native::event::ServerNotification;
use devo_protocol::native::ids::{ItemId as NativeItemId, SessionId as NativeSessionId};
use devo_protocol::{AcpClientCapabilities, AcpInitializeParams};
use devo_server::{
    ActiveTurnSteeringState, ApprovalDecisionValue, ApprovalRequestPayload, ApprovalResponseParams,
    ApprovalScopeValue, ClientRequest, ItemDeltaKind, PendingServerRequestContext, ProtocolError,
    ProtocolErrorCode, ServerRequestKind, SteerInputRecord, TurnKind, item_delta_notification,
};
use pretty_assertions::assert_eq;

fn sample_native_session(
    title: Option<&str>,
    title_state: SessionTitleState,
) -> devo_protocol::native::session::Session {
    use devo_protocol::native::ids::SessionId as NativeSessionId;
    use devo_protocol::native::model::{ModelBinding, PermissionProfile};
    use devo_protocol::native::session::{
        Session, SessionActivity, SessionSettings, SessionStatus,
    };
    use devo_protocol::native::usage::{SessionUsage, UsageTotals};

    let now = Utc::now();
    Session {
        id: NativeSessionId::new(),
        version: 1,
        cwd: ".".into(),
        additional_directories: Vec::new(),
        parent: None,
        fork_from_id: None,
        at_turn_id: None,
        ephemeral: false,
        created_at: now,
        status: SessionStatus::Idle,
        activity: SessionActivity::Idle,
        flags: Vec::new(),
        archived: false,
        active_turn_id: None,
        queued_count: 0,
        title: title.map(str::to_string),
        title_state,
        model: ModelBinding {
            provider: "unknown".into(),
            model: "claude-sonnet".into(),
            variant: None,
            reasoning_effort: None,
        },
        settings: SessionSettings {
            permission_profile: PermissionProfile::AutoReview,
            reasoning_effort: None,
            mode: Some("build".into()),
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
        usage: SessionUsage {
            total: UsageTotals::default(),
            by_purpose: Vec::new(),
            legacy: None,
            updated_at: now,
        },
    }
}

#[test]
fn acp_initialize_params_accept_documented_minimal_shape() {
    let params: AcpInitializeParams =
        serde_json::from_value(serde_json::json!({ "protocolVersion": 1 }))
            .expect("deserialize ACP initialize params");

    assert_eq!(
        params,
        AcpInitializeParams {
            protocol_version: 1,
            client_capabilities: AcpClientCapabilities::default(),
            client_info: None,
            meta: None,
        }
    );
}

#[test]
fn approval_response_roundtrip() {
    let payload = ApprovalResponseParams {
        session_id: SessionId::new(),
        turn_id: TurnId::new(),
        approval_id: "approval-1".into(),
        decision: ApprovalDecisionValue::Approve,
        scope: ApprovalScopeValue::Session,
    };

    let json = serde_json::to_string(&payload).expect("serialize");
    let restored: ApprovalResponseParams = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(payload, restored);
}

#[test]
fn user_input_serializes_tagged_shape() {
    let input = devo_protocol::native::item::UserInput::Skill {
        name: "rust-docs".into(),
    };

    let json = serde_json::to_string(&input).expect("serialize");
    assert!(json.contains("\"type\":\"skill\""));
}

#[test]
fn protocol_error_uses_spec_code_strings() {
    let payload = ProtocolError {
        code: ProtocolErrorCode::NotInitialized,
        message: "handshake incomplete".into(),
        data: serde_json::json!({}),
    };

    let json = serde_json::to_string(&payload).expect("serialize");
    assert!(json.contains("NotInitialized"));
}

#[test]
fn server_request_payload_roundtrip() {
    let payload = ApprovalRequestPayload {
        request: PendingServerRequestContext {
            request_id: "req-1".into(),
            request_kind: ServerRequestKind::ItemPermissionsRequestApproval,
            session_id: SessionId::new(),
            turn_id: Some(TurnId::new()),
            item_id: None,
        },
        approval_id: "approval-1".into(),
        action_summary: "run shell command".into(),
        justification: "writes files".into(),
        resource: Some("ShellExec".into()),
        available_scopes: vec!["once".into(), "turn".into(), "session".into()],
        path: None,
        host: None,
        target: Some("echo hi".into()),
        command_pattern: Some(vec!["echo".into(), "*".into()]),
        command_prefix: None,
    };

    let json = serde_json::to_string(&payload).expect("serialize");
    let restored: ApprovalRequestPayload = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(payload, restored);
}

#[test]
fn steering_state_preserves_queue_order() {
    let first = SteerInputRecord {
        item_id: ItemId::new(),
        received_at: Utc::now(),
        input: vec![devo_protocol::native::item::UserInput::Text {
            text: "first".into(),
        }],
    };
    let second = SteerInputRecord {
        item_id: ItemId::new(),
        received_at: Utc::now(),
        input: vec![devo_protocol::native::item::UserInput::Text {
            text: "second".into(),
        }],
    };

    let state = ActiveTurnSteeringState {
        turn_id: TurnId::new(),
        turn_kind: TurnKind::Regular,
        pending_inputs: VecDeque::from([first.clone(), second.clone()]),
    };

    assert_eq!(state.pending_inputs[0], first);
    assert_eq!(state.pending_inputs[1], second);
}

#[test]
fn item_delta_notification_wires_expected_method() {
    let notification = item_delta_notification(
        ItemDeltaKind::AgentMessageDelta,
        NativeSessionId::new(),
        NativeItemId::new(),
        0,
        "hi",
    );

    let (method, _) =
        devo_protocol::native::wire_projector::wire_from_server_notification(&notification);
    assert_eq!(method, "item/assistantMessage/delta");
}

#[test]
fn request_envelope_keeps_method_and_id() {
    let request = ClientRequest {
        id: serde_json::json!(1),
        method: "session/new".into(),
        params: serde_json::json!({"cwd":"C:/repo"}),
    };

    let json = serde_json::to_string(&request).expect("serialize");
    assert!(json.contains("\"method\":\"session/new\""));
    assert!(json.contains("\"id\":1"));
}

#[test]
fn session_metadata_updated_notification_serializes_expected_method() {
    let notification = ServerNotification::SessionMetadataUpdated {
        session: Box::new(sample_native_session(
            Some("Renamed session"),
            SessionTitleState::Final(SessionTitleFinalSource::UserRename),
        )),
    };

    let (method, _) =
        devo_protocol::native::wire_projector::wire_from_server_notification(&notification);
    assert_eq!(method, "session/metadataUpdated");
}

#[test]
fn session_compaction_notifications_serialize_expected_methods() {
    let session = sample_native_session(Some("Compacting session"), SessionTitleState::Unset);
    let turn_id = TurnId::new();
    let started = ServerNotification::ContextCompactionStarted {
        session_id: session.id,
        turn_id,
        trigger: devo_protocol::native::item::CompactionTrigger::Manual,
    };
    let completed = ServerNotification::ContextCompactionCompleted {
        session_id: session.id,
        turn_id,
        item_id: ItemId::new(),
    };
    let failed = ServerNotification::ContextCompactionFailed {
        session_id: SessionId::new(),
        message: "boom".into(),
    };

    assert_eq!(
        devo_protocol::native::wire_projector::wire_from_server_notification(&started).0,
        "context/compactionStarted"
    );
    assert_eq!(
        devo_protocol::native::wire_projector::wire_from_server_notification(&completed).0,
        "context/compactionCompleted"
    );
    assert_eq!(
        devo_protocol::native::wire_projector::wire_from_server_notification(&failed).0,
        "context/compactionFailed"
    );
}

/// Trace: L2-DES-APP-009
/// Verifies: compaction lifecycle notifications wire as canonical
/// context/compactionStarted and context/compactionCompleted.
#[test]
fn compaction_lifecycle_events_project_to_native_notifications() {
    let session = sample_native_session(Some("Compacting session"), SessionTitleState::Unset);

    let turn_id = TurnId::new();
    let (method, value) = devo_protocol::native::wire_projector::wire_from_server_notification(
        &ServerNotification::ContextCompactionStarted {
            session_id: session.id,
            turn_id,
            trigger: devo_protocol::native::item::CompactionTrigger::Manual,
        },
    );
    assert_eq!(method, "context/compactionStarted");
    assert_eq!(value["trigger"].as_str(), Some("manual"));
    assert_eq!(value["turnId"].as_str(), Some(turn_id.to_string().as_str()));

    let item_id = ItemId::new();
    let (method, value) = devo_protocol::native::wire_projector::wire_from_server_notification(
        &ServerNotification::ContextCompactionCompleted {
            session_id: session.id,
            turn_id,
            item_id,
        },
    );
    assert_eq!(method, "context/compactionCompleted");
    assert_eq!(value["itemId"].as_str(), Some(item_id.to_string().as_str()));
}

/// Trace: L2-DES-APP-008, L2-DES-CONV-002
/// Verifies: the canonical session/metadata/update contract shape (patch
/// payload with SessionSettings + expectedVersion) round-trips on the wire.
#[test]
fn native_session_metadata_update_params_roundtrip() {
    use devo_protocol::native::model::PermissionProfile;
    use devo_protocol::native::rpc_session::SessionMetadataUpdateParams;
    use devo_protocol::native::session::SessionSettings;

    let params: SessionMetadataUpdateParams = serde_json::from_value(serde_json::json!({
        "sessionId": "00000000-0000-0000-0000-000000000001",
        "expectedVersion": 3,
        "settings": {
            "permissionProfile": "fullAccess",
            "sandboxProfile": "workspace",
            "reasoningEffort": "high"
        }
    }))
    .expect("deserialize canonical params");
    assert_eq!(params.expected_version, 3);
    let settings = params.settings.clone().expect("settings present");
    assert_eq!(
        settings.permission_profile,
        Some(PermissionProfile::FullAccess)
    );
    assert_eq!(settings.sandbox_profile.as_deref(), Some("workspace"));
    assert_eq!(settings.reasoning_effort, Some("high".to_string()));
    let roundtripped: SessionMetadataUpdateParams =
        serde_json::from_value(serde_json::to_value(&params).expect("serialize canonical params"))
            .expect("re-deserialize canonical params");
    assert_eq!(roundtripped, params);

    // A minimal settings object defaults the unset fields.
    let minimal: SessionSettings = serde_json::from_value(serde_json::json!({
        "permissionProfile": "default"
    }))
    .expect("minimal settings");
    assert_eq!(minimal.permission_profile, PermissionProfile::Default);
    assert_eq!(minimal.sandbox_profile, None);
    assert_eq!(minimal.reasoning_effort, None);
    assert_eq!(minimal.mode, None);
    assert_eq!(minimal.effective_context_window, None);
}

/// Trace: L2-DES-CONV-002, L2-DES-APP-008
/// Verifies: the settings patch is partial (only present fields change) and
/// `expectedVersion: 0` is the documented no-precondition escape.
#[test]
fn native_session_settings_patch_is_partial() {
    use devo_protocol::native::rpc_session::SessionSettingsPatch;

    let patch: SessionSettingsPatch =
        serde_json::from_value(serde_json::json!({ "sandboxProfile": "strict" }))
            .expect("partial patch deserializes");
    assert_eq!(patch.permission_profile, None);
    assert_eq!(patch.sandbox_profile.as_deref(), Some("strict"));
    assert_eq!(patch.reasoning_effort, None);
    assert_eq!(patch.mode, None);
    assert_eq!(patch.effective_context_window, None);
    assert_eq!(
        serde_json::to_value(&patch).expect("serialize"),
        serde_json::json!({ "sandboxProfile": "strict" }),
        "absent fields stay absent on the wire"
    );
}

/// Trace: L2-DES-APP-008
/// Verifies: the canonical task domain wire shapes (task/start kind-tagged
/// params, task verb params) serialize as specified by DD-7.
#[test]
fn native_task_start_params_kind_tagged_wire_shape() {
    use devo_protocol::native::ids::SessionId;
    use devo_protocol::native::rpc_turn::TaskStartParams;

    let process = TaskStartParams::Process {
        session_id: SessionId::from_string("00000000-0000-0000-0000-000000000001".into()),
        command: "ls".into(),
        cwd: None,
        idempotency_key: "k-1".into(),
    };
    let value = serde_json::to_value(&process).expect("serialize process params");
    assert_eq!(
        value,
        serde_json::json!({
            "kind": "process",
            "sessionId": "00000000-0000-0000-0000-000000000001",
            "command": "ls",
            "idempotencyKey": "k-1"
        })
    );
    let roundtripped: TaskStartParams =
        serde_json::from_value(value).expect("deserialize process params");
    assert_eq!(roundtripped, process);

    let agent: TaskStartParams = serde_json::from_value(serde_json::json!({
        "kind": "agent",
        "sessionId": "00000000-0000-0000-0000-000000000001",
        "input": [{ "type": "text", "text": "hi" }],
        "idempotencyKey": "k-2"
    }))
    .expect("deserialize agent params");
    assert!(matches!(agent, TaskStartParams::Agent { .. }));
}

/// Trace: L2-DES-APP-008
/// Verifies: the canonical goal domain wire shapes (goal/set with ifExists,
/// goal transition params) round-trip.
#[test]
fn native_goal_params_wire_shapes() {
    use devo_protocol::native::rpc_session::{
        GoalIfExists, SessionGoalSetParams, SessionGoalTransitionParams,
    };

    let set = SessionGoalSetParams {
        session_id: devo_protocol::native::ids::SessionId::from_string(
            "00000000-0000-0000-0000-000000000001".into(),
        ),
        objective: "ship it".into(),
        token_budget: Some(1000),
        if_exists: GoalIfExists::Replace,
        idempotency_key: "g-1".into(),
    };
    let value = serde_json::to_value(&set).expect("serialize goal/set params");
    assert_eq!(value["ifExists"], serde_json::json!("replace"));
    let roundtripped: SessionGoalSetParams =
        serde_json::from_value(value).expect("deserialize goal/set params");
    assert_eq!(roundtripped, set);

    let transition: SessionGoalTransitionParams = serde_json::from_value(serde_json::json!({
        "sessionId": "00000000-0000-0000-0000-000000000001",
        "expectedGoalId": "goal_00000000-0000-0000-0000-000000000002"
    }))
    .expect("deserialize transition params");
    assert_eq!(
        transition.expected_goal_id.as_str(),
        "goal_00000000-0000-0000-0000-000000000002"
    );
}

/// Trace: L2-DES-APP-010
/// Verifies: first-party Native turn lifecycle uses identity `turn/started`
/// wire (session/event projector deleted).
#[test]
fn native_turn_started_identity_wire() {
    use devo_protocol::native::wire_projector::wire_from_server_notification;

    let session_id = SessionId::new();
    let turn_id = TurnId::new();
    let notification = ServerNotification::TurnStarted {
        turn: Box::new(devo_protocol::native::turn::Turn {
            id: turn_id,
            session_id,
            sequence: 1,
            kind: devo_protocol::native::turn::TurnKind::Regular,
            status: devo_protocol::native::turn::TurnStatus::InProgress,
            model: devo_protocol::native::model::ModelBinding {
                provider: "unknown".into(),
                model: "m".into(),
                variant: None,
                reasoning_effort: None,
            },
            collaboration_mode: None,
            started_at: Utc::now(),
            completed_at: None,
            error: None,
            usage: None,
        }),
    };
    let (method, params) = wire_from_server_notification(&notification);
    assert_eq!(method, "turn/started");
    assert!(params.get("turn").is_some());
}

/// Trace: L2-DES-AUTH-001
/// Verifies: provider/authStale notification serializes with camelCase providerId.
#[test]
fn provider_auth_stale_notification_wire() {
    let notification = ServerNotification::ProviderAuthStale {
        provider_id: "anthropic".to_string(),
        reason: Some("oauth expired".to_string()),
    };
    let value = serde_json::to_value(&notification).expect("serialize authStale");
    assert_eq!(value["method"], "provider/authStale");
    assert_eq!(value["params"]["providerId"], "anthropic");
    assert_eq!(value["params"]["reason"], "oauth expired");
    let roundtrip: ServerNotification =
        serde_json::from_value(value).expect("deserialize authStale");
    assert_eq!(roundtrip, notification);
}
