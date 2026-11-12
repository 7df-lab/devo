# TUI workspace trust

When starting `devo` in a directory for the first time, review the displayed
canonical directory and type **yes** only if you trust its contents. Enter,
EOF, or any other answer declines; no Native server or session is started by
that TUI before acceptance. `--yolo` does not skip this prompt.

Project instructions and configuration can influence the agent, model endpoints,
and tools/MCP processes running with your account's permissions. Workspace trust
is an admission decision, **not a sandbox**. Acceptance does not change the
permission preset, sandbox profile, or approval rules. An existing full-access
configuration remains full access.

## Remembering and revoking trust

Approvals live in `$DEVO_HOME/workspace-trust/` (normally
`~/.devo/workspace-trust/`), one JSON file per canonical absolute directory.
Trust applies to that exact directory, not parents, children, or siblings.
Symlinks resolve to their real destination. Replacing contents at the same path
does not reset trust: review repositories you update or replace.
Delete the corresponding JSON record (its `cwd` identifies the folder), or the
whole `workspace-trust` directory, to ask again on the next launch. Exit existing
TUI processes first; their accepted workspaces remain trusted for their lifetime.

If `DEVO_HOME` is inside the workspace, approval is session-only. Devo does not
read or write persisted trust there, so a repository cannot supply its own
approval record. Only point `DEVO_HOME` at configuration you already trust:
user-level environment/configuration and the installed application are part of
the trusted computing base, not covered by this prompt.

## Resume and switching sessions

`devo resume` checks the startup directory before launching the server, then
reads the saved session's directory and checks it before resuming. New sessions,
imports, and watched sessions check admission before activation. Forks derive from
an already admitted current session and check the forked session before switching. Once the fullscreen
UI is running, switching to an untrusted directory fails with an instruction to
exit and launch `devo` in that directory. This avoids competing terminal prompts
and silently trusting a different workspace through the session list.

## Scope

This prompt protects the **interactive TUI entrypoint**, including direct
`apps/tui` startup. CLI startup logging/update checks use user-level settings
rather than reading project configuration before admission. The server loads
project settings when the admitted session is created/resumed.

It is not a server authorization mechanism: Desktop, headless `devo prompt`,
Native/ACP clients, and independently running agents are not gated by the TUI's
local trust store. Existing remote work is not stopped by declining a TUI
attachment. Trust does not revoke tools' access to other directories. Use the
permission/sandbox configuration for that restriction.

Reference: [Claude Code security](https://code.claude.com/docs/en/security)
describes an interactive workspace trust prompt separately from tool permissions
and sandboxing. Devo's exact-directory scope and session-switch behavior above
are its own documented choices, not a claim of identical Claude Code internals.
