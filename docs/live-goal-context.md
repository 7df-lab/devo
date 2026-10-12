# Goal changes during a turn

Goal state is owned by the host. A running query owns a checked-out conversation
copy, so updating the session actor alone does not update its prompt context.

Host goal changes now publish an override through the active turn's shared
`live_goal` handle and await the short actor command. The query applies that
override before assembling its next model request. An unset override preserves
the turn-start goal; a cleared goal removes the active-goal reminder. This is
goal control state, independent of session settings and model selection.

MergeTurn preserves the actor's goal mirror so an older conversation copy cannot
overwrite a goal changed during the turn. Python `goal.complete()` still returns
normally, allowing the model to report completion and usage before ending its
turn. It does not force an interrupt or change keyboard behavior.

The goal completion integration test captures both model requests: the first
contains the active-goal reminder, and the request after successful completion
contains the tool result without that reminder. Existing lifecycle tests cover
host interruption, pause, resume and continuation admission.
