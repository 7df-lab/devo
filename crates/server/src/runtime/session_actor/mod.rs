// Per-session actor: single-writer for durable session state.
//
// Unbounded turn I/O (model streams, tools) runs on a spawned task with a
// [`turn_working::TurnWorkingSet`]. The actor mailbox stays short-command only;
// turn results re-enter through `MergeTurn`. Control-plane Arcs (queues,
// TurnInlineState, cancel tokens) let mid-turn RPCs avoid mailbox hops.

mod actor_loop;
pub(crate) mod approval_scope;
mod commands;
mod handle;
pub(crate) mod registry;
pub(crate) mod snapshots;
pub(crate) mod state;
mod turn;
mod turn_inline;
mod turn_working;

pub(crate) use handle::SessionHandle;
pub(crate) use state::SessionActorState;
pub(crate) use turn_working::TurnWorkingSet;

/// Whether replacing conversation state also invalidates its live Python runtime.
#[derive(Clone, Copy)]
pub(crate) enum KernelReplacementPolicy {
    Preserve,
    Reset,
}
