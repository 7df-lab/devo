# Desktop branch selection

The new-chat branch picker scopes its branch reads, status check, confirmation
dialogs, and checkout admission to the selected project directory. Selecting a
different project unmounts that scope, including when returning to the same
directory later. A late status response cannot start a checkout or display a
confirmation after its scope has gone away.

One selection owns the picker from its status check through the dirty-worktree
and active-session confirmations to checkout completion. Repeated selections and
confirmation clicks do not start duplicate operations during that lifetime.
Canceling either confirmation releases the picker without changing Git state.
Closing-dialog controls are disabled during their exit animation. Any delayed
dismissal or confirmation is checked against its original request and phase;
it cannot release an already-dispatched checkout or cancel a later confirmation.
Sessions that become active while the status check is pending still trigger the
active-session warning.

Branch-list errors appear in the popover with an explicit Retry control. Closing
the popover invalidates that read; reopening starts a fresh one, and an older
success or failure cannot replace it. Status and checkout failures remain visible
until dismissed or another branch selection starts. They are never automatically
retried.

A checkout or stash already sent to Electron cannot be canceled by changing
projects. It may complete in the original directory, but its late result will
not change the new project's picker or invoke its branch-change callback. This
UI guard is not a cross-window Git transaction, a server-side operation lock, or
exactly-once execution across remounts. No Git backend or stash semantics change.

Regression coverage uses the real React/Base UI picker with a scripted Electron
backend. It covers navigation, close/reopen, repeated clicks, both confirmation
paths, cancellation, failures, explicit retries, and local/remote branch names.
It does not claim packaged Electron visual or live-repository checkout testing.
