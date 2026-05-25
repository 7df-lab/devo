---
artifact_id: L2-DES-TUI-004
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-TUI-004 — Streaming Transcript And State

## Purpose

Refine TUI streaming rendering, transcript review, and state visibility requirements into a concrete display model for live and completed session activity.

## Background / Context

The TUI must show progress while work is happening and preserve a readable audit trail afterward. Model output, tool execution, approvals, questions, errors, and background processes arrive as ordered server events. The TUI should render those events promptly without treating transient live state as durable transcript truth.

## Source Requirements

- `L1-REQ-TUI-002` requires timely streaming of assistant text, reasoning summaries, tool starts, tool output deltas, and completion states.
- `L1-REQ-TUI-003` requires a durable, readable, scrollable transcript.
- `L1-REQ-TUI-004` requires visible idle, generating, tool, waiting, interrupted, failed, completed, background process, and input-mode states.
- `L1-REQ-TUI-007` requires stable layout during streaming and resize.
- `L1-REQ-APP-004` requires actionable diagnostics.
- `L1-REQ-TOOL-005` requires visibility and manual stop access for background processes.
- `L2-DES-APP-003` defines server-client event payloads.
- `L2-DES-CONV-001` defines durable transcript records.
- `L2-DES-TOOL-001` defines tool lifecycle and result summaries.
- `L2-DES-APP-004` defines observability fields used by diagnostic display.
- `L2-DES-CONTEXT-002` defines compaction lifecycle records and user-visible compaction notices.

## Design Requirement

The TUI should render from a transcript projection plus a live overlay:

```text
Durable transcript projection
        +
Live server-client events
        +
Local composer state
        ↓
Visible TUI frame
```

The durable transcript projection provides stable review content. The live overlay provides in-progress streaming text, running tool output, waiting prompts, spinners, and active process state. When the server finalizes an item, the live overlay should reconcile into the durable transcript cell.

## Shell Placement Boundary

`L2-DES-TUI-002` owns the overall shell placement of the transcript viewport, working indicator, composer, and bottom status line. This document owns the rendering rules for cells inside that transcript viewport and for the live working indicator that appears immediately below the transcript while a turn is active.

Placement contract:

- Transcript cells render in the transcript viewport.
- The live working indicator renders after the latest transcript content and before the composer.
- The composer and bottom status line remain outside the transcript viewport.
- Entering full-screen alternate transcript mode, such as through `Ctrl+T`, may use the same transcript cell renderers with expanded output limits.

Compact shell placement:

```text
<transcript viewport>
┃ Thought: The fix is isolated to escaped quote handling.
┃ I will patch the parser and run the focused suite.
┃ Running  cargo test parser::quoted -- --nocapture

⠋ Working · 12s

<composer>
┃ Ask Devo

<status>
  Build · deepseek-v4-pro high  ↑420[cached 300 71%]  ↓12  ▰▰▱▱▱▱▱▱▱▱  20%  190k/950k
```

## Transcript Cell Types

The TUI should use explicit cell types rather than a single generic text block.

| Cell Type | Purpose | Durable |
|---|---|---|
| User message | User-submitted content and mentions. | Yes |
| Assistant message | Final or streaming assistant response. | Yes |
| Reasoning summary | User-visible reasoning summary where available and allowed. | Yes, when emitted as visible item |
| Explore tool group | Read, glob, and grep/search activity grouped for scanning. | Yes |
| File mutation tool | Create/write and edit/apply-patch activity with diff preview. | Yes |
| Shell running cell | Active shell command state. | No, reconciles into shell result |
| Shell result cell | Completed shell command output summary and folded output. | Yes |
| Tool call | Other tool starts, arguments preview, approval state, command description. | Yes |
| Tool output | Other tool result summary, bounded output, status, redaction state. | Yes |
| Running tool overlay | Live output and spinner for an active tool. | No, reconciles into tool cells |
| Approval prompt | Pending or resolved approval request. | Yes |
| Question prompt | Pending or resolved Plan Mode question. | Yes |
| Error | Recoverable or terminal failure with recovery hint. | Yes |
| Plan update | Current plan item changes. | Yes |
| Background process | Tracked process state and stop affordance. | Yes for lifecycle events, live for recent output |
| Context/usage status | Token usage, context pressure, compaction notice. | Yes when recorded, live in header/status |
| Working indicator | Active turn indicator between transcript and composer. | No |
| Completed turn summary | Final assistant turn metadata after completion. | Yes |

## Transcript Area Visual Design

The transcript area is a vertical list of cells. Cells fall into three main categories:

1. User message cells.
2. Assistant message cells.
3. Tool message cells.

The left marker `┃` is the main visual anchor for transcript cells. For assistant and tool cells, it marks the first visible line of a logical cell. For user-message cells, which are background-band surfaces, it may repeat on each user-authored content line when the message has multiple lines. The marker column should align consistently across transcript content, use color to distinguish role/state, and remain readable without color.

### User Message Cells

User message cells begin with a left `┃` rendered in the theme primary foreground color. The user cell is a background band. For single-line messages, the marker appears only on the content line. For multi-line messages, each user-authored content line may repeat the marker.

A single-line user message renders as a three-row band:

```text

┃ Fix the parser regression and run the focused tests.

```

Rules:

- The top padding row, content row, and bottom padding row share the same background span.
- Only content rows carry the primary-colored `┃` marker.
- User message text uses normal foreground color.
- Multi-line user messages keep the same background band and preserve author-entered line breaks.
- Multi-line user-authored content lines may repeat `┃`; the top and bottom padding rows must not render `┃`.

Multi-line example:

```text

┃ Refactor the parser in three steps:
┃ 1. isolate quoted-value parsing
┃ 2. add regression tests
┃ 3. run the focused suite

```

### Assistant Reasoning And Reply Cells

Assistant cells also begin with a single `┃`, but they do not use a background band. Wrapped continuation lines align with the assistant text column and do not repeat the marker.

Reasoning cells:

- Appear above the reply cell for the same assistant turn.
- Use muted foreground for reasoning body text.
- Begin with `Thinking:` while reasoning is streaming.
- Change to `Thought:` after the reasoning item is complete.
- Render `Thinking:` and `Thought:` in italic styling when supported.
- Use a distinct muted/accent color for the label so it differs from primary text but remains visually quiet.

Streaming reasoning example:

```text
┃ Thinking: Inspecting the parser branch and matching it against the
  existing quoted-value tests.
```

Completed reasoning example:

```text
┃ Thought: The failure is isolated to escaped quote handling, so the
  smallest fix is a parser branch update plus a focused regression test.
```

Reply cells:

- Appear below reasoning cells.
- Use normal white foreground.
- Stream incrementally as assistant text arrives.
- Begin with a single `┃` and do not use a background band.

Streaming reply example:

```text
┃ The parser accepts quoted values, but the escape branch currently
  treats a backslash before a quote as ordinary text. I will update the
  branch and add a regression test before running the focused suite.
```

### Completed Turn Summary

After an assistant turn completes, the assistant reply cell should show a compact completed-turn summary.

```text
┃ The focused parser tests now pass. I also added a regression test for
  escaped quotes.

  ▣ Build · DeepSeek V4 Pro · 2.1s
```

Rules:

- `▣` uses the theme primary foreground color.
- `Build` is the active mode label for the completed turn. It may be `Plan` when the turn ran in Plan Mode.
- `DeepSeek V4 Pro` is the display model name.
- `2.1s` is total turn duration.
- The summary appears only after completion and should not duplicate the live working indicator.

## Tool Message Visual Design

Tool message cells begin with a single `┃` and should communicate the tool family, target, and outcome without requiring raw logs.

### Explore Tools: Read, Glob, Grep

`read`, `glob`, and `grep` are grouped under `Explore`.

Rules:

- Consecutive `read` calls may be grouped under a single `Explore` title.
- Each `read` call renders on its own line. Multiple read targets must not be merged into one `Read` line.
- The `read` target must include the file parameter.
- `glob` and `grep` each render as their own line even when consecutive.
- `glob` is file-pattern search. `grep` is content search.

Example:

```text
┃ Explore
  ┗ Read  crates/core/src/query.rs
    Read  crates/core/src/session/turn.rs
    Glob  crates/**/*.rs
    Grep  "execute_turn" crates/core
```

If another `read` of `crates/core/src/query.rs` arrives immediately after the group above, it is still a distinct tool call and should render as its own `Read` line.

### File Mutation Tools: Create And Edit

`write` and `apply_patch` render as file mutation cells with a diff preview.

Rules:

- `write` renders as `Create <path>`.
- `apply_patch` renders as `Edit <path>`.
- If the target is inside the workspace, the path is workspace-relative.
- If the target is outside the workspace, the path is absolute.
- A blank line separates the title from the diff.
- Diff content should use a git-diff-like layout.
- Diff lines should render on a diff background.
- Added, removed, and metadata lines should be distinguishable by color and symbol even when the diff background is present.

Create example:

```text
┃ Create crates/parser/src/quoted.rs

  diff --git a/crates/parser/src/quoted.rs b/crates/parser/src/quoted.rs
  new file mode 100644
  +pub fn parse_quoted(input: &str) -> Result<String, ParseError> {
  +    todo!("parse escaped quotes")
  +}
```

Edit example:

```text
┃ Edit crates/parser/src/lib.rs

  diff --git a/crates/parser/src/lib.rs b/crates/parser/src/lib.rs
  @@ parse_value
  -        return parse_bare_value(input);
  +        return parse_quoted_or_bare_value(input);
```

### Shell Tool: Running And Run Cells

Shell calls render as two related cells:

1. `Running`: one-line active command state.
2. `Run`: completed command result with compressed output.

Running example:

```text
┃ Running  cargo test parser::quoted -- --nocapture
```

Completed output example:

```text
┃ Run      cargo test parser::quoted -- --nocapture     failed  2.3s
┗ output   64 lines hidden, 12 shown                    Ctrl+T for full transcript
    test parser::quoted_empty ... ok
    test parser::quoted_escape ... FAILED
    assertion failed: expected escaped quote handling
```

Rules:

- The `Running` cell updates while the process is active.
- The `Run` cell replaces or follows the `Running` cell when the command completes.
- `Run` output is compressed by default.
- The `┗` relationship marker connects the command title to its output summary.
- Pressing `Ctrl+T` enters full-screen alternate mode for reviewing the full transcript and full output.
- The compressed output should show enough lines to explain the result and must indicate hidden line counts.

### Context And Compaction Cells

Context and compaction status cells render in the transcript area so the user can later review when context was compacted.

Compaction lifecycle cells must use these exact visible labels:

```text
┃ Manual Compaction Started
┃ Automatically Compaction Started
┃ Compaction Done
```

Rules:

- `Manual Compaction Started` appears when compaction starts because the user requested it, such as through `/compact`.
- `Automatically Compaction Started` appears when compaction starts because context pressure crossed the configured threshold.
- `Compaction Done` appears when a compaction event completes successfully and the active context snapshot has been updated.
- These cells are transcript-area status cells, not assistant messages, user messages, or model-visible context content.
- Inline rendering should preserve the exact label text. Counts, token estimates, or summary inspection affordances may be available in an expanded detail view, but must not change the inline label.
- On replay, durable compaction records should project back into the same transcript-area status cells.

## Active Turn Working Indicator

When the current turn has not completed, the TUI should show a live working indicator between the transcript area and the bottom composer.

Example:

```text
⠋ Working · 12s

┃ Ask Devo
```

Rules:

- The left side is an animated spinner using this frame sequence: `⠋`, `⠙`, `⠹`, `⠸`, `⠼`, `⠴`, `⠦`, `⠧`, `⠇`, `⠏`.
- The text `Working` identifies the active turn state.
- A dot separates the state from elapsed time.
- Elapsed time is compact and may use seconds, minutes, hours, or days, such as `12s`, `3m`, `2h`, or `1d`.
- The working indicator is live-only and disappears when the turn completes.
- After completion, the completed-turn summary replaces the need for the working indicator.
- The shell layout reserves placement for this indicator; transcript rendering provides the content and state transition semantics.

## Live Streaming Layout

Live streaming examples should use the same transcript cell grammar as completed content. The TUI should not introduce a second table-like visual language for live state.

Normal streaming assistant response:

```text
┃ The parser accepts quoted values, but the escape branch currently
  treats a backslash before a quote as ordinary text. I will update...
```

Reasoning summary:

```text
┃ Thinking: Inspect parser branch -> add regression -> patch escape handling -> run tests.
```

Running tool with output deltas:

```text
┃ Running  cargo test parser::quoted -- --nocapture
  ┗ output  +18 lines
      test parser::quoted_empty ... ok
      test parser::quoted_escape ... FAILED
```

Tool completed with folded output:

```text
┃ Run      cargo test parser::quoted -- --nocapture       failed  2.3s
  ┗ output  64 lines hidden, 12 shown                     Ctrl+T for full transcript
      test parser::quoted_empty ... ok
      test parser::quoted_escape ... FAILED
      assertion failed: expected escaped quote handling
```

Approval wait:

```text
┃ Approval required                            waiting
  apply_patch wants to modify 2 files.
  [Approve] [Deny] [Details]
```

Background process:

```text
┃ Background  npm run dev                      running  03:14
  ┗ output     http://localhost:3000 ready
    recent output +5 lines                     [Stop]
```

## State Mapping

The TUI should map canonical server events into visible state.

| Server Event | TUI State |
|---|---|
| `turn_started` | New turn row and running status. |
| `item_started` assistant | Assistant cell appears immediately. |
| `item_content_update` | Existing cell updates before final completion. |
| `item_completed` | Live cell becomes completed transcript cell. |
| `tool_call_started` | Tool row appears even before final output exists. |
| `tool_call_updated` | Tool progress/output preview updates. |
| `tool_call_completed` | Tool result summary and status become durable review content. |
| `approval.requested` | Approval prompt appears and bottom status shows waiting reason. |
| `question.requested` | Question prompt appears and bottom status shows waiting for answer. |
| `background_process_updated` | Background process strip and transcript state update. |
| `usage_updated` | Header/context or turn usage display updates. |
| `context_updated` with compaction start | Transcript area shows `Manual Compaction Started` or `Automatically Compaction Started` based on compaction trigger source. |
| `context_updated` with compaction completion | Transcript area shows `Compaction Done` and context pressure display updates. |
| `context_updated` without lifecycle change | Context pressure display updates. |
| `error_reported` | Error cell appears with phase and recovery action. |
| `turn_status_changed` | Header/status and terminal turn cell update. |

The TUI must not wait for all parallel tools to finish before showing a started or updated sibling tool when the server has emitted the sibling event.

## Timeliness Requirements

The TUI should be event-driven and repaint promptly after meaningful server events.

Rules:

- Assistant deltas should update the visible assistant cell before the final response completes.
- Tool start should be visible before tool completion.
- Tool output deltas should update the visible tool cell before final completion.
- Parallel tool events should be independently visible.
- Approval and question waits should interrupt ambiguous "running" display with a specific waiting reason.
- The TUI may coalesce frequent deltas to avoid flicker, but coalescing must not make active work appear frozen during normal operation.
- Live Markdown rendering should preserve readable partial output and avoid corrupting completed transcript layout.

## Transcript Review

Completed transcript content should support audit and recovery.

Rules:

- Completed turns remain reviewable after live rendering finishes.
- Tool calls should show command or tool summary, status, timing, and bounded result.
- Approval and question cells should show the prompt and final resolution.
- Error cells should show phase, concise message, recoverability, and recovery action where available.
- Long output should be folded, truncated, or referenced rather than rendered without bound.
- Omitted content must be marked with a visible line count, byte count, or content reference.
- Scrollback should preserve logical item boundaries so users can find relevant work.

## Active Work Strip

The active work strip should summarize the most important current state without replacing transcript content.

Examples:

```text
⠋ Working · 12s
  running   cargo test parser::quoted         output +24 lines
  waiting   approval for apply_patch          modifies 2 files
  context   81% near limit                    compaction available
  cleanup   interrupted turn                  1 background process running
```

Priority:

1. Approval or question waiting state.
2. Active tool or background process state.
3. Model generation state.
4. Context pressure or compaction state.
5. Idle or ready state.

## Failure And Interruption Display

Failure and interruption states should be explicit transcript events.

Example:

```text
┃ Interrupted                                  user requested
  completed  read src/parser.rs
  stopped    cargo test parser::quoted
  next       resume, edit previous message, or submit a new request
```

Rules:

- Terminal turn state must remain visible after the live spinner stops.
- Partial assistant/tool content should remain visible if it was already emitted.
- Recovery actions should be shown when the server provides them.
- The TUI should distinguish failed, interrupted, canceled, and completed states.

## Markdown And Wrapping

Markdown rendering should be readable in both live and completed cells.

Rules:

- Live Markdown may use a tolerant incremental renderer.
- Completed Markdown may be re-rendered from final content for better formatting.
- Code blocks should preserve indentation and wrap or scroll according to transcript policy.
- Tables may degrade to preformatted text or simplified columns in narrow terminals.
- Links and file references should remain visible as text in terminals that cannot open them directly.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-002 | 1 | specs/L1/L1-REQ-TUI-002-streaming.md | Defines live streaming behavior for assistant text, reasoning summaries, tool starts, tool deltas, and completion. |
| refines | L1-REQ-TUI-003 | 1 | specs/L1/L1-REQ-TUI-003-transcript.md | Defines transcript cell types, review behavior, folding, and durable/live reconciliation. |
| refines | L1-REQ-TUI-004 | 1 | specs/L1/L1-REQ-TUI-004-state-visibility.md | Defines visible state mapping for idle, generating, tools, approvals, questions, failures, interruptions, and background processes. |
| related-to | L1-REQ-TUI-007 | 1 | specs/L1/L1-REQ-TUI-007-responsive-layout-readability.md | Streaming and transcript rendering must remain stable across resize and narrow widths. |
| related-to | L1-REQ-TOOL-005 | 1 | specs/L1/L1-REQ-TOOL-005-background-process-management.md | Background process state and stop controls are rendered in the TUI. |
| related-to | L1-REQ-APP-004 | 1 | specs/L1/L1-REQ-APP-004-observability.md | User-facing diagnostics and waiting reasons inform state display. |
| related-to | L2-DES-APP-003 | 1 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Server-client events drive live rendering. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Durable transcript records are the replay source. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | Tool lifecycle, command descriptions, and result summaries feed tool cells. |
| related-to | L2-DES-APP-004 | 1 | specs/L2/app/L2-DES-APP-004-observability-architecture.md | Diagnostic fields provide recovery and phase display. |
| related-to | L2-DES-CONTEXT-002 | 1 | specs/L2/context/L2-DES-CONTEXT-002-context-compaction.md | Compaction lifecycle records render as transcript-area status cells. |
| specified-by | TBD | TBD | specs/L3/tui/TBD.md | L3 behavior has not been authored yet. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial streaming transcript and state visibility design. |
| 1 | 2026-05-23 | Human | Refinement | Added concrete transcript-area visual design for user, assistant, tool, shell, working, and completed-turn cells. |
| 1 | 2026-05-23 | Human | Refinement | Clarified shell placement boundary between transcript viewport, working indicator, composer, and status line. |
| 1 | 2026-05-23 | Human | Refinement | Clarified that `┃` is a single leading marker for each logical cell, not a full-cell rail. |
| 1 | 2026-05-23 | Human | Refinement | Defined the working spinner frame sequence and changed Explore read rendering to one line per read call. |
| 1 | 2026-05-23 | Human | Refinement | Updated live streaming examples to reuse the transcript cell visual grammar. |
| 1 | 2026-05-23 | Human | Refinement | Clarified that multi-line user-message background bands may repeat `┃` on content lines while padding rows remain background-only. |
| 1 | 2026-05-23 | Human | Refinement | Reconciled active work and interruption examples with the current transcript and working-indicator visual grammar. |
| 1 | 2026-05-25 | Human | Refinement | Added exact transcript-area labels for manual compaction start, automatic compaction start, and compaction completion. |
