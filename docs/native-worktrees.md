# Native worktree management

Desktop uses four Native RPCs. External protocol adapters are unchanged.

| Method | Parameters | Result |
| --- | --- | --- |
| `workspace/worktree/list` | `cwd` | `worktrees`: directory, name, branch |
| `workspace/worktree/create` | `cwd`, `name` | directory, name, branch |
| `workspace/worktree/remove` | `cwd`, `directory` | directory |
| `workspace/worktree/reset` | `cwd`, `directory` | directory |

List returns registered linked checkouts, excluding the primary checkout.
Create branches from HEAD into `$DEVO_HOME/worktrees/<name>-<unique suffix>`.
Names allow 1–64 ASCII letters, numbers, hyphens, and underscores. Creation
completes after Git has prepared the checkout; Desktop no longer polls a
placeholder endpoint or sends a Bash environment-copy snippet on Windows.
Monorepo sessions preserve their subdirectory relative to the Git root.

Remove and reset require a registered, clean linked checkout, and reject the
primary checkout, untracked files, and working sessions in that directory.
Remove keeps the Git branch. Reset targets the repository's default branch.
Settings shows confirmation and retains errors for retry. Mutations serialize
separately from session actor locks. Git explicitly receives null stdin so it cannot block the Native stdio reader
on Windows. Git operations allow 60 seconds; Desktop
allows 90 seconds for worktree RPCs. Create is not idempotent: retrying creates
another uniquely named checkout.
