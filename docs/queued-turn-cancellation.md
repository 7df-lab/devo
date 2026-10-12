# Queued-turn cancellation

A drained follow-up is a normal in-flight turn. Before publishing its start,
queue admission registers its Native turn metadata and cancellation token in
the active-turn registry. Both queue-start paths run it on a dedicated task
whose abort handle belongs to that same registry.

`session/interrupt` can then find and cancel the queued turn, just as it does
an ordinary turn. Cancellation goes through normal finalization and merge;
the dedicated abort handle remains available for timeout recovery. The
post-turn chain still waits for the queued task to finish before returning.
No session-map lock is retained while awaiting actor commands in admission.

The TUI's immediate abort display is not proof of server cancellation. QA
requires the matching Native terminal turn to be interrupted and pending
agent-requested compaction to be cleared without a later snapshot.

`queued_turn_interrupt.rs` gates provider responses to ensure a follow-up is
queued before the first turn ends, then interrupts its pending model stream.
It also checks a responsive mid-turn session read and retains an idle queue
push as a control for ordinary cancellation. No timing sleeps establish the
race. Ctrl+O, Escape key bindings and model-selection behavior are unchanged.
