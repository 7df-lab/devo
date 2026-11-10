# Desktop commit and push recovery

The Review panel's **Commit & push** dialog still uses the existing Electron Git
operations: stage all changes, commit in the selected directory, then push its
current branch. It does not add a new Git backend or change remote selection.

## Failure recovery

- A successful scan with at least one changed file is required before committing.
  Scan failures are visible and offer **Retry scan**, rather than treating an
  unreadable repository as zero changed files and enabling a blind commit.
- A reported commit failure leaves the message editable for an explicit retry.
- After a confirmed successful commit, the dialog locks the message and refreshes
  the Review panel once. If push fails, **Retry push** retries only the push. It
  never stages more changes, creates another commit, or refreshes as if another
  commit succeeded.
- A rejected commit request has an uncertain outcome. The dialog asks the user
  to check Git history and prevents another commit in that visit. An uncertain
  push result preserves the known local commit and asks the user to check the
  remote before explicitly retrying the push.
- Successful completion offers **Done** and retains the existing short automatic
  close. Its timer cannot close a later dialog visit.

## Ownership and interruption

Each open visit owns its scan, pending operation, feedback, and timer. Repeated
submits are synchronously blocked. Cancel, Escape, outside dismissal, and the
close control cannot dismiss a pending commit or push. Closing controls are
disabled during the exit animation, and stale handlers cannot act on a new visit.

External navigation or unmount still invalidates the visit. A commit already
dispatched may finish locally, but its late result does not refresh a different
project or start a follow-up push. A push already dispatched may finish in the
original directory; its late result cannot update or close another dialog.

Push recovery is limited to the current open visit. Dismissing it, navigating, or
reloading discards that UI state; reopening scans fresh and does not automatically
push earlier commits. This is not an unpushed-commit browser, branch pinning, a
cross-window Git lock, or exactly-once execution. The existing push operation
uses the directory's current branch when it executes. A scan is not an atomic
snapshot of files later staged by Git.

Regression tests run real React/Base UI components against a scripted Electron
backend. They cover known/uncertain failures, explicit retries, duplicate clicks,
scan interruption, navigation, exit animations, stale handlers, and automatic
close ownership. They do not perform real commits or remote pushes, or claim
packaged Electron visual validation.
