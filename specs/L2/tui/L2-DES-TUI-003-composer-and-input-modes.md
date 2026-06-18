---
artifact_id: L2-DES-TUI-003
revision: 4
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-TUI-003 — Composer And Input Modes

## Purpose

Refine TUI composer, terminal command prefix, and session-local input mode requirements into a concrete interaction design.

## Background / Context

The composer is the user's main control surface. It must handle normal chat input, multi-line input, command discovery, TUI-only shell command entry, and Plan Mode input without surprising the user or breaking Unicode/IME text entry.

Session-local input modes affect how the TUI interprets the next composer submission. They are distinct from session-level agent modes.

## Source Requirements

- `L1-REQ-TUI-001` requires reliable text entry, multi-line input, intentional submit, command discovery, and session-local input modes.
- `L1-REQ-TUI-006` requires discoverable and intentional command invocation from the TUI.
- `L1-REQ-TUI-008` requires leading `!` input to enter Shell Mode and execute through the terminal command capability.
- `L1-REQ-TUI-009` requires Default Input Mode, Shell Mode, Plan Mode, and bottom status line labels for `BUILD`, `PLAN`, and `SHELL`.
- `L1-REQ-TUI-007` requires the composer and bottom status line to remain readable and non-overlapping.
- `L1-REQ-CLIENT-001` requires Unicode, IME, and wide-character safety.
- `L1-REQ-AGENT-005` defines agent-level Plan Mode behavior.
- `L1-REQ-TOOL-002` defines the command execution capability used by Shell Mode.
- `L2-DES-TUI-002` defines the shell regions occupied by composer and status line.
- `L2-DES-TUI-008` defines composer, popup, status-line, and slash-command style tokens.
- `L2-DES-CLIENT-001` defines localization and Unicode readiness.

## Design Requirement

The composer should be a stable bottom-region editor with explicit submission semantics, visible non-default input modes, and predictable prefix handling.

The TUI should support these session-local input modes:

| Input Mode | Purpose | Status Label |
|---|---|---|
| Default Input Mode | Normal build/task input. | `BUILD` |
| Shell Mode | Terminal command input routed to the program's command execution capability. | `SHELL` |
| Plan Mode | Plan-oriented input governed by agent-level Plan Mode behavior. | `PLAN` |

## Style System Boundary

The composer owns input behavior and command interpretation. The visual treatment of the input band, slash-command rows, mode colors, popup spacing, selected rows, and status symbols should come from `L2-DES-TUI-008`.

Composer-specific token use:

- Input band background: `surface.inputBand`.
- Empty hint: `text.muted`.
- User-entered text: `text.primary`.
- Prompt marker `┃`: active input mode color for the bottom composer only.
- Matched slash command token: `core.primary`.
- Slash command parameter hint: `text.muted`.
- Mode label colors: `mode.build`, `mode.plan`, and `mode.shell`.

## Composer Layout

Default composer:

```text

┃ Ask Devo

  BUILD · deepseek-v4-pro high  ↑0[cached 0 0%]  ↓0  ▱▱▱▱▱▱▱▱▱▱  0%  0/950k
```

Multi-line composer:

```text

┃ Refactor the parser in three steps:
┃ 1. isolate quoted value parsing
┃ 2. add regression tests
┃ 3. run the focused suite

  BUILD · deepseek-v4-pro high  ↑0[cached 0 0%]  ↓0  ▰▰▱▱▱▱▱▱▱▱  20%  190k/950k
```

Shell Mode composer:

```text

┃ cargo test parser::quoted -- --nocapture

  SHELL · deepseek-v4-pro high  ↑420[cached 300 71%]  ↓12  ▰▰▱▱▱▱▱▱▱▱  20%  190k/950k
```

Plan Mode composer:

```text

┃ Plan the migration steps before changing files.

  PLAN · deepseek-v4-pro high  ↑420[cached 300 71%]  ↓12  ▰▰▱▱▱▱▱▱▱▱  20%  190k/950k
```

Rules:

- Composer content lines use a left `┃` marker in the active input mode color. Build uses the current blue/cyan color, Plan uses purple, and Shell uses orange.
- The composer is rendered as a full-width input band: one padding line above the content, the content lines, and one padding line below the content share the same background span.
- For multi-line input, each user-entered content line may repeat the `┃` marker. The top and bottom padding rows must keep the input-band background but must not render `┃`.
- `Ask Devo` is the empty-input hint. It uses muted grey text and disappears as soon as the user types content.
- User-entered input replaces the hint and uses normal input foreground styling.
- The status label is the first visible field in the bottom status line.
- `BUILD`, `PLAN`, and `SHELL` must use distinct colors.
- The bottom status line appears below the composer.
- Composer height may grow for multi-line input within configured bounds, then scroll internally or show a line count.

## Bottom Status Line

The bottom status line has this conceptual shape:

```text
  BUILD · deepseek-v4-pro high  ↑0[cached 0 0%]  ↓0  ▱▱▱▱▱▱▱▱▱▱  0%  0/950k
```

Fields:

- `BUILD`, `PLAN`, or `SHELL`: current TUI input mode, rendered as the first visible status-line field. `BUILD` is the normal default label. `PLAN` and `SHELL` replace it when those session-local input modes are active.
- `deepseek-v4-pro`: current model display name. The supported model slug is a fallback only for recovery or invalid configuration states where a display name is unavailable.
- `high`: current reasoning effort.
- `↑0[cached 0 0%]`: input token count, cached input token count, and input cache hit rate.
- `↓0`: output token count.
- `▱▱▱▱▱▱▱▱▱▱`: context-window usage bar.
- `0%`: context-window usage percentage.
- `0/950k`: context-window usage and effective context-window length.

The status line should derive model, reasoning, token, cache, and context values from server-confirmed usage and context events where available. If a value is unavailable, estimated, or redacted, the status line should use a compact marker defined by L3 rather than inventing an exact value.

## Mode Cycling

The TUI toggles Build and Plan input modes with `Shift+Tab`. Shell Mode is entered only through the leading `!` prefix.

Rules:

- `Shift+Tab` toggles `Build <-> Plan`.
- `Shift+Tab` must not enter Shell Mode from either Build Mode or Plan Mode.
- If Shell Mode is already active, `Shift+Tab` may leave Shell Mode and return to Build Mode.
- Shell Mode is entered only by typing bare `!` as the first composer character. `!cmd` from Build Mode runs a one-shot shell command and returns to Build Mode.
- The cycle is session-local TUI state. It does not change the session-level agent mode.
- The active mode label must update immediately as the first field on the bottom status line.
- `Build` uses the default build/input color, `Plan` uses the plan color, and `Shell` uses the shell color.
- The shortcut is handled before normal composer text editing so it cannot insert a tab character into the draft.

## Submission Semantics

The TUI should separate text editing from submission.

Rules:

- Plain submit sends the current composer content according to active input mode.
- Supported modified-enter input inserts a newline instead of submitting.
- If a terminal cannot report the required key sequence, the TUI may expose an alternate newline action through command discovery or documented keybinding.
- Empty input should not create a normal chat turn.
- The composer must preserve the submitted content exactly as entered, subject only to intentional mode-specific parsing.
- Submission should use client-generated ids so reconnect or retry does not duplicate messages.

This L2 design does not mandate exact keybindings because terminal event support differs. The L3 design should define required keybindings and fallbacks for supported terminals.

## Prefix Handling

The TUI-specific terminal command prefix is `!` at the first character of composer input.

Rules:

- If the first character of composer input is `!`, the TUI enters Shell Mode.
- Leading whitespace before `!` does not trigger Shell Mode. This keeps pasted text and indented examples from becoming commands unexpectedly.
- A literal normal-chat message beginning with `!` should be escapable by prefixing a backslash, for example `\!important`. The backslash is removed before normal chat submission.
- Typing bare `!` as the first composer character enters Shell Mode and clears the `!` from the editor.
- Submitting input that starts with `!` followed by command text from Build Mode runs the command once and returns the composer to Build Mode.
- While Shell Mode is active, submitted composer content is treated as command text until the user changes mode.

## Shell Mode Execution

Shell Mode turns composer content into a command execution request.

Rules:

- The command text is the composer content after the leading `!` prefix or the Shell Mode editor content.
- Shell Mode command execution must use the program's terminal command capability, not an unmanaged client-local shell.
- Each Shell Mode submission should start a fresh one-shot PTY command through the server `command/exec` API and render `command/exec/outputDelta` notifications.
- `!cmd` from Build Mode should use the same one-shot `command/exec` path and then return to Build Mode.
- If no Devo session exists yet, Shell Mode and `!cmd` should omit `session_id`, pass an explicit `cwd`, and must not call `session/start`, emit session activation, or apply session permissions only to execute a command.
- After a Devo session exists, Shell Mode command execution may include that `session_id`, but it still must not create a new session for shell execution.
- Shell Mode must respect workspace, permission policy, safety, privacy, and sandbox constraints.
- Shell Mode results should appear in the transcript as direct command execution output with `UserShell` source attribution and bounded display.
- Shell Mode process completions should use `▣ Shell` for the summary instead of the active model display name.
- If approval is required, the TUI should show the approval prompt and keep Shell Mode state understandable.
- Command output should be summarized or folded when long.
- Failed commands should show status, exit code where available, and a natural-language result summary.

Example Shell Mode flow:

```text
User types:

! cargo test parser::quoted

TUI state:

▌ $ cargo test parser::quoted

⠋ Working · ⏱ 2s

┃ Ask Devo

  SHELL · deepseek-v4-pro high  ↑420[cached 300 71%]  ↓12  ▰▰▱▱▱▱▱▱▱▱  20%  190k/950k

After completion:

▌ $ cargo test parser::quoted
  └ Test failed: parser::quoted_escape expected escaped quote handling.

  ▣ Shell · 2.1s
```

## Plan Mode Input

Plan Mode is a session-local TUI input mode that activates agent-level Plan Mode behavior for submitted input.

Rules:

- Plan Mode must be visible in the bottom status line.
- Plan Mode does not change the session-level agent mode.
- Submitted Plan Mode input must be marked so the server applies Plan Mode rules.
- For the v1 TUI/server integration, Plan Mode is prompt-level and advisory: the server appends hidden model context instructing the agent not to modify files or perform implementation work.
- The v1 integration does not require broad per-tool runtime refactoring. Hard mutation blocking remains a later policy/tool-gating concern.
- The TUI must not present Plan Mode as permission to make changes.
- Leaving Plan Mode returns the composer to Build Mode.

Plan Mode can be entered by `Shift+Tab` mode cycling. Slash command or command palette entry points may be added by later designs.

## Command Discovery

The composer should provide command discovery without replacing typed text unexpectedly.

Command discovery behavior:

- Typing `/` in an empty composer opens the slash-command list.
- The composer line remains visible as `┃ /` while the list is open.
- `!` enters Shell Mode as defined above.
- Slash command suggestions should appear directly below the composer prompt line.
- The slash-command list has an eight-row visible height.
- Slash command rows render with a two-character prefix area: `> ` for the focused row and two spaces for non-focused rows.
- Up and Down arrow keys move the focused selection marked by `>`.
- Enter confirms the focused selection.
- Esc closes suggestions and preserves typed input where safe.
- The focused row uses the theme primary foreground color for both the command name and description.
- In non-focused rows, the command name uses normal white foreground and the description uses muted foreground.
- Slash-command discovery rows normally do not use `●` because slash commands are actions rather than already enabled options.
- Selecting a suggestion must either invoke the command or open the relevant flow.

Open slash-command list:

```text
┃ /

> /theme        switch the UI theme
  /model        choose the active model
  /compact      compact the current session context
  /resume       resume a saved chat
  /goal         set or view the goal for a long-running task
  /new          start a new chat
  /status       show current session configuration and token usage
  /permissions  choose what Devo is allowed to do
```

Full slash-command list:

```text
  /theme        switch the UI theme
  /model        choose the active model
  /compact      compact the current session context
  /resume       resume a saved chat
  /goal         set or view the goal for a long-running task
  /new          start a new chat
  /status       show current session configuration and token usage
  /permissions  choose what Devo is allowed to do
  /clear        clear the current transcript
  /btw          start a side conversation in an ephemeral fork
  /exit         exit Devo
```

The eight-row visible list shows the first eight matching commands by default. When there are more than eight matches, the selection may scroll through the full list while preserving the two-character left padding and row color rules.

Slash-command inline rendering:

- When composer input begins with `/` and matches an existing slash command, the matched command token uses the theme primary foreground color.
- Parameters or placeholder text following the matched slash command use muted foreground color.
- If the typed slash command does not match an existing command, the composer should not apply matched-command coloring.
- Inline command coloring is presentational only. Command parsing and validation still happen when the user confirms or submits the command.
- Submitted user history cells apply the same matched-command coloring to a leading recognized `/name` token, including rows restored or constructed without composer text-element metadata.
- For `/goal`, free-form text after the command is the objective. Pressing Enter submits that objective directly to the goal command instead of opening a budget prompt or create wizard.

Example matched slash command with parameter hint:

```text
┃ /btw <your side conversation message>

  BUILD · deepseek-v4-pro high  ↑0[cached 0 0%]  ↓0  ▱▱▱▱▱▱▱▱▱▱  0%  0/950k
```

In the rendered TUI, `/btw` uses the primary foreground color and `<your side conversation message>` uses muted foreground color.

Command purposes:

- `/theme`: switch the UI theme.
- `/model`: choose the active model.
- `/compact`: compact the current session context.
- `/resume`: resume a saved chat.
- `/goal`: set or view the goal for a long-running task.
- `/new`: start a new chat.
- `/status`: show current session configuration and token usage.
- `/permissions`: choose what Devo is allowed to do.
- `/clear`: clear the current transcript.
- `/btw`: start a side conversation in an ephemeral fork.
- `/exit`: exit Devo.

Command-specific L2 designs:

| Command | Design Artifact |
|---|---|
| `/theme` | `L2-DES-TUI-CMD-001` |
| `/model` | `L2-DES-TUI-CMD-002` |
| `/compact` | `L2-DES-TUI-CMD-003` |
| `/resume` | `L2-DES-TUI-CMD-004` |
| `/goal` | `L2-DES-TUI-CMD-010` |
| `/new` | `L2-DES-TUI-CMD-005` |
| `/status` | `L2-DES-TUI-CMD-006` |
| `/permissions` | `L2-DES-TUI-CMD-007` |
| `/clear` | `L2-DES-TUI-CMD-008` |
| `/btw` | `L2-DES-TUI-CMD-011` |
| `/exit` | `L2-DES-TUI-CMD-012` |

## Unicode And IME Constraints

Composer editing must be text-model based rather than byte-position based.

Rules:

- Cursor movement should respect grapheme clusters and display columns.
- CJK and other wide characters should not corrupt wrapping or cursor placement.
- IME composition should not submit partial composition text.
- Non-ASCII text should remain intact through local editing, submission, server transport, transcript display, and replay.
- The composer should rely on `L2-DES-CLIENT-001` for cross-client localization and Unicode rules.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-001 | 1 | specs/L1/L1-REQ-TUI-001-composer.md | Defines composer layout, multi-line behavior, submission semantics, command discovery, and Unicode constraints. |
| refines | L1-REQ-TUI-006 | 1 | specs/L1/L1-REQ-TUI-006-command-discovery-control.md | Defines slash-command trigger behavior, list height, keyboard navigation, row styling, and the initial command list. |
| refines | L1-REQ-TUI-008 | 1 | specs/L1/L1-REQ-TUI-008-terminal-command-prefix.md | Defines leading `!` prefix behavior, Shell Mode execution, escaping, and command result display. |
| refines | L1-REQ-TUI-009 | 1 | specs/L1/L1-REQ-TUI-009-session-input-modes.md | Defines Default, Shell, and Plan Mode behavior plus bottom status line labels. |
| related-to | L1-REQ-TUI-007 | 1 | specs/L1/L1-REQ-TUI-007-responsive-layout-readability.md | Composer and bottom status line must remain readable across terminal sizes. |
| related-to | L1-REQ-CLIENT-001 | 1 | specs/L1/L1-REQ-CLIENT-001-localization-readiness.md | Composer input must preserve Unicode, IME, and wide-character text. |
| related-to | L1-REQ-AGENT-005 | 1 | specs/L1/L1-REQ-AGENT-005-plan-mode.md | Plan Mode input must trigger agent-level planning-only behavior. |
| related-to | L1-REQ-TOOL-002 | 1 | specs/L1/L1-REQ-TOOL-002-tools.md | Shell Mode uses the built-in command execution capability. |
| related-to | L2-DES-TUI-002 | 1 | specs/L2/tui/L2-DES-TUI-002-modern-tui-shell-layout.md | Defines the shell regions used by composer and status line. |
| related-to | L2-DES-TUI-008 | 1 | specs/L2/tui/L2-DES-TUI-008-style.md | Defines composer band, popup list, slash-command, and status-line style rules. |
| related-to | L2-DES-CLIENT-001 | 1 | specs/L2/client/L2-DES-CLIENT-001-localization-readiness.md | Defines shared Unicode and localization design constraints. |
| specified-by | L3-BEH-TUI-001 | 2 | specs/L3/tui/L3-BEH-TUI-001-layout-composer-input.md | L3 defines composer layout, input mode handling, prefix handling, and bottom status line behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial composer, Shell Mode, Plan Mode, and command discovery design. |
| 1 | 2026-05-23 | Human | Refinement | Added `Ask Devo` prompt band styling, `Build` default status label, distinct mode colors, and bottom token/cache/context status fields. |
| 1 | 2026-05-23 | Human | Refinement | Added slash-command popup behavior, eight-row command list, selection styling, keyboard navigation, and initial command catalog. |
| 1 | 2026-05-23 | Human | Refinement | Added inline slash-command coloring for matched command tokens and muted parameter hints. |
| 1 | 2026-05-23 | Human | Refinement | Clarified that multi-line composer input repeats `┃` on content lines while top and bottom padding rows remain background-only. |
| 1 | 2026-05-23 | Human | Refinement | Reconciled shell-mode examples and slash-command catalog with the current TUI visual grammar and approved command list. |
| 1 | 2026-05-23 | Human | Refinement | Linked the shared command catalog to command-specific L2 design artifacts. |
| 1 | 2026-05-23 | Human | Refinement | Removed `/diff` from the slash-command catalog. |
| 1 | 2026-05-23 | Human | Refinement | Changed `/btw` from active-turn injection to a side conversation in an ephemeral fork. |
| 1 | 2026-05-23 | Human | Refinement | Added `/goal` as the TUI entry point for Ralph Loop goals. |
| 1 | 2026-05-25 | Human | Refinement | Clarified that `/goal <objective>` submits the following text as the objective without a default budget prompt. |
| 1 | 2026-05-25 | Assistant | Refinement | Added composer-level handling guidance for direct `/goal` objective submission. |
| 2 | 2026-05-26 | Assistant | Revision | Linked composer and command discovery visual treatment to the shared TUI style system. |
| 3 | 2026-05-26 | Human | Refinement | Updated navigable slash-command lists so `>` marks the focused row and `●` is reserved for enabled options in choice lists. |
| 4 | 2026-05-27 | Human | Refinement | Removed `/onboard` from slash-command discovery because onboarding is entered through startup CLI arguments. |
| 4 | 2026-06-08 | Assistant | Refinement | Defined `Shift+Tab` Build/Plan toggling, leftmost uppercase mode labels, Shell-only `!` entry behavior, one-shot PTY-backed Shell execution including sessionless startup commands, prompt-only Plan Mode v1 behavior, active-mode composer marker color, direct user-shell output, and `▣ Shell` process summaries. |
| 5 | 2026-06-18 | Assistant | Refinement | Clarified that submitted user history cells also highlight a leading recognized slash-command token, even when no composer text-element metadata is available. |
