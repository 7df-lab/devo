//! Native protocol types: the single domain definition shared by
//! persistence (rollout JSONL) and all four wire surfaces (Native, ACP,
//! External, A2A).
//!
//! These types are the schema truth source (`devo-api-design/README.md` §3):
//! wire JSON is camelCase, times are RFC 3339 UTC, IDs are opaque strings.
//! They are introduced alongside the legacy protocol types (05 P0/P1) and do
//! not replace them until the migration phases land.

pub mod error;
pub mod event;
pub mod goal;
pub mod id_bridge;
pub mod ids;
pub use ids::{
    EventId, GoalId, ItemId, JobId, OpaqueId, QueueItemId, RestorePlanId, RunId, SessionId,
    SubscriptionId, TurnId,
};
pub mod item;
pub mod methods;
pub mod model;
pub mod notification_bus;
pub mod page;
pub mod patch;
pub mod pi;
pub mod plan_parse;
pub mod queue;
pub mod rpc_admin;
pub mod rpc_catalog;
pub mod rpc_schedule;
pub mod rpc_search;
pub mod rpc_session;
pub mod rpc_turn;
pub mod rpc_workspace;
pub mod rpc_worktree;
pub mod session;
pub mod turn;
pub mod usage;
pub mod wire_projector;

pub use id_bridge::{uuid_from_item_id, uuid_from_session_id, uuid_from_turn_id};
pub use notification_bus::{
    notification_legacy_session_id, notification_method_name, notification_native_session_id,
    notification_touches_session_activity,
};

pub use plan_parse::{
    plan_entries_from_plan_text, plan_entries_from_plan_text_or_single,
    plan_entries_from_update_plan_json, plan_entry_from_json, plan_step_status_from_str,
};
