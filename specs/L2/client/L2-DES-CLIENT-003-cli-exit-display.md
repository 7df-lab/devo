---
artifact_id: L2-DES-CLIENT-003
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-06-23
---

# L2-DES-CLIENT-003 — CLI Exit Session Display

## Purpose

Define the terminal output printed by the CLI after the interactive TUI session ends, so the user can see session usage and know how to resume the session later.

## Background / Context

When the user exits the interactive TUI (via `/exit` or `Ctrl+C` double-press), the TUI safely restores terminal modes and returns control to the shell. Before the CLI process exits, it prints a summary of the just-ended session: token usage and a resume command. This gives the user actionable information at the shell prompt without requiring them to re-enter the program just to find the session identifier.

A session is created when the first user message is submitted (per L1-REQ-CONV-001). If the user launches the TUI and exits without sending any message, no session was created and no exit display should be shown.

## Source Requirements

- `L1-REQ-LLM-001` requires token usage and cached-token information to be exposed where available.
- `L1-REQ-LLM-003` requires recording input, output, and cached input token usage.
- `L1-REQ-CONV-001` requires the user to be able to resume an existing session.
- `L1-REQ-TUI-005` requires terminal mode restoration on exit and safe shell prompt handoff.
- `L2-DES-TUI-005` defines the terminal lifecycle state machine including safe exit and shell prompt handoff.
- `L2-DES-CONV-001` defines the session data model that carries token usage aggregates.

## Design Decisions

### DD-1: Exit display is a CLI responsibility, not a TUI responsibility

The TUI renders inside the terminal using alternate screens and inline regions. After exit, the terminal returns to normal shell mode. Printing usage information at this point is a CLI concern — it uses ordinary `println!` into the restored terminal.

**Decision**: The TUI returns an `AppExit` struct carrying the session identifier, turn count, and token aggregates. The CLI formats and prints the exit display from this struct. The TUI itself never prints to stdout after terminal restore.

### DD-2: Token usage display uses ANSI color where the terminal supports it

Token usage is multi-dimensional (total, non-cached input, cached input, output). Color distinguishes the dimensions at a glance, making the line scannable. When color is disabled (non-TTY stdout, `NO_COLOR`), the same information is rendered without ANSI escape sequences.

**Decision**: The token usage line uses:
- **bright cyan** for the aggregate `total` value
- **bright green** for the non-cached `input` value
- **yellow** for the `cached` count and label, parenthesized when non-zero
- **bright magenta** for the `output` value

The color scheme is applied via ANSI SGR escape sequences. The same values are printed without escape sequences when `color_enabled` is false.

### DD-3: Session resume hint uses dimmed instructional text with a bright command

The resume hint has two distinct visual roles: explanatory text that sets context, and an actionable command the user can copy-paste. Dimming the explanatory text while keeping the command bright creates a natural visual hierarchy.

**Decision**: The resume line uses:
- **dimmed** text for `To continue this session, run` (ANSI `SGR 2`)
- **bright white** for the `devo resume <session-id>` command (ANSI `SGR 97`)

The bright white color for the command draws the eye to the actionable part. The dimmed prefix provides context without competing visually.

### DD-4: No exit display is shown when no session was used

Entering the TUI and exiting immediately without sending any message creates no session (per L1-REQ-CONV-001: "a new session is created when the first user message for that session is submitted"). Showing a resume hint for a non-existent session would be misleading.

**Decision**: The exit display is suppressed when the session was never used — specifically, when `turn_count` is zero. This covers the common case of launching the program, browsing config or model selection, and exiting before starting any actual work.

## Display Format

### Token Usage Line

The token usage line is printed on a single line, unconditionally when token data is available:

```
Token usage: total=1,889,658 input=111,103 (+ 1,758,464 cached) output=20,091
```

**Format spec**:

| Component | Format | Example | Color |
|-----------|--------|---------|-------|
| Label `total=` | Literal | `total=` | default |
| Total value | Separator-grouped integer | `1,889,658` | bright cyan |
| Label `input=` | Literal | `input=` | default |
| Non-cached input | Separator-grouped integer | `111,103` | bright green |
| Cached parenthetical | ` (+ <value> cached)` when `cache_read_tokens > 0` | `(+ 1,758,464 cached)` | yellow |
| Label `output=` | Literal | `output=` | default |
| Output value | Separator-grouped integer | `20,091` | bright magenta |

**Computation rules**:

- `total = total_tokens`, where the session aggregate uses provider-reported `total_tokens` for each invocation when available and otherwise falls back to `input_tokens + output_tokens`.
- `non_cached_input = total_input_tokens - total_cache_read_tokens` (saturating subtraction)
- The cached parenthetical is omitted when `cache_read_tokens` is zero.
- All numeric values use thousands-separator formatting (e.g., `1,889,658`).
- Separately reported `reasoning_output_tokens` are breakdowns only and are not added to `total`.

**Suppression**: The token usage line is suppressed when both `total` and `cache_read_tokens` are zero — this occurs when no work was performed.

### Session Resume Line

The session resume line is printed on a single line after the token usage line (when present):

```
To continue this session, run devo resume 019e3b7c-b19c-7f93-bd7a-de19f460dfa9
```

**Format spec**:

| Component | Content | Color |
|-----------|---------|-------|
| Instructional prefix | `To continue this session, run` | dimmed |
| Space separator | ` ` | default |
| Resume command | `devo resume <session-id>` | bright white |

**Suppression**: The resume line is suppressed when `session_id` is `None` (no session was created) or when `turn_count` is zero (session was created and immediately discarded without use).

### Complete Example

When the user completes work with multiple turns filled with model calls:

```
Token usage: total=1,889,658 input=111,103 (+ 1,758,464 cached) output=20,091
To continue this session, run devo resume 019e3b7c-b19c-7f93-bd7a-de19f460dfa9
```

When the user launches and immediately exits without sending any message:

```
(no output)
```

When the user completes work with no cached tokens:

```
Token usage: total=5,200 input=3,100 output=2,100
To continue this session, run devo resume 019e3b7c-b19c-7f93-bd7a-de19f460dfa9
```

## Data Contract

### AppExit

The TUI produces an `AppExit` struct at termination. This is the sole data channel between the TUI and the CLI exit display:

```
AppExit {
    session_id: Option<SessionId>,
    turn_count: usize,
    total_input_tokens: usize,
    total_output_tokens: usize,
    total_tokens: usize,
    total_cache_read_tokens: usize,
}
```

| Field | Source in TUI | Updated by |
|-------|---------------|------------|
| `session_id` | `InteractiveLoopState::session_id` | `SessionActivated`, `SessionSwitched` worker events |
| `turn_count` | `InteractiveLoopState::turn_count` | `TurnFinished`, `TurnFailed` worker events |
| `total_input_tokens` | `InteractiveLoopState::total_input_tokens` | `TurnFinished`, `TurnFailed`, `UsageUpdated`, `SessionCompacted`, `SessionSwitched` |
| `total_output_tokens` | `InteractiveLoopState::total_output_tokens` | Same as above |
| `total_tokens` | `InteractiveLoopState::total_tokens` | Same as above |
| `total_cache_read_tokens` | `InteractiveLoopState::total_cache_read_tokens` | Same as above |

Token values are running totals accumulated over the session lifetime. The `total_tokens` field carries the display total already selected by server/runtime accounting: provider total when present, otherwise `input_tokens + output_tokens`. They are surfaced by the server process through `TurnUsageUpdated` events, which the TUI worker translates to `WorkerEvent::UsageUpdated`, `WorkerEvent::TurnFinished`, and `WorkerEvent::TurnFailed`.

### Integration Points

The flow from TUI exit to CLI display:

```
User triggers exit (Ctrl+C double-press or /exit)
  → LoopAction::ClearAndExit
    → tui.shutdown_terminal_safe()  (clear screen, leave alt screen)
    → TerminalRestoreGuard::restore()  (disable raw mode)
    → worker.shutdown()
    → return AppExit { session_id, turn_count, ... }
  → CLI run_agent() resolves AppExit
  → CLI exit_messages(&exit, color_enabled)
    → format_token_usage_line()  (token display)
    → resume hint line             (session resume command)
  → println! each line
```

## Color Semantics

The color palette is chosen to be visually distinct on both light and dark terminal backgrounds:

| ANSI Code | Effect | Used For | Rationale |
|-----------|--------|----------|-----------|
| `1;36` | Bold Cyan | `total` | Aggregate: neutral, high-salience for the headline number |
| `1;32` | Bold Green | `input` | Cost input: green suggests acceptable/expected flow |
| `1;33` / `33` | Bold/Normal Yellow | `cached` value and label | Cache benefit: yellow suggests optimization / savings |
| `1;35` | Bold Magenta | `output` | Output: magenta is visually distinct from input green |
| `2` | Dim | `To continue this session, run` | Instructional text: dimmed to de-emphasize |
| `97` | Bright White | `devo resume <id>` | Command: bright white draws attention to the actionable string |

These codes are standard ANSI SGR sequences compatible with all modern terminal emulators. No 24-bit or 256-color codes are used, ensuring compatibility with basic terminal environments.

## CLI Integration

The `exit_messages` function is called after `run_agent()` returns, in every CLI code path that launches the interactive TUI:

| CLI path | Calls exit_messages? |
|----------|---------------------|
| `devo` (no subcommand, new session) | Yes |
| `devo --onboard` | Yes |
| `devo resume <id>` | Yes |
| `devo prompt "..."` | No (non-interactive) |
| `devo doctor` | No (diagnostic) |
| `devo server` | No (daemon) |

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| refines | L1-REQ-LLM-001 | 1 | specs/L1/L1-REQ-LLM-001-token-efficiency.md | Exposes token usage and cached-token information at session exit. |
| refines | L1-REQ-LLM-003 | 1 | specs/L1/L1-REQ-LLM-003-observability.md | Surfaces input, output, and cached-read token usage to the user after session ends. |
| refines | L1-REQ-CONV-001 | 1 | specs/L1/L1-REQ-CONV-001-session-lifecycle.md | Provides the session resume command at exit, enabling the user to resume later. |
| related-to | L1-REQ-TUI-005 | 1 | specs/L1/L1-REQ-TUI-005-terminal-lifecycle-safety.md | Exit display occurs after the TUI has safely restored terminal modes. |
| related-to | L2-DES-TUI-005 | 1 | specs/L2/tui/L2-DES-TUI-005-terminal-lifecycle-safety.md | TUI lifecycle defines safe exit and shell prompt handoff before CLI prints. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Session data model carries the token aggregates surfaced in exit display. |
| related-to | L2-DES-LLM-003 | 1 | specs/L2/llm/L2-DES-LLM-003-model-usage-observability.md | Model usage observability design defines streaming token tracking that feeds the exit display. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Turn usage updates flow from server to TUI via the client-server protocol. |
| specified-by | L3-BEH-TUI-003 | 1 | specs/L3/tui/L3-BEH-TUI-003-terminal-lifecycle-safety.md | L3 defines safe terminal handoff before CLI exit display. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial CLI exit session display design. |
| 1 | 2026-06-23 | Assistant | Refinement | Added `total_tokens` to the AppExit contract and aligned total display with conservative provider-total fallback semantics. |
