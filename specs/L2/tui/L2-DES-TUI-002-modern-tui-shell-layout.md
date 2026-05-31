---
artifact_id: L2-DES-TUI-002
revision: 3
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-TUI-002 — Modern TUI Shell Layout

## Purpose

Refine the terminal user interface, transcript, state visibility, and responsive layout requirements into a concrete modern TUI shell design.

## Background / Context

The TUI is the first interactive client. It must feel like a capable terminal-native work surface, not a decorative chat page. The layout should be dense, calm, readable, and stable while the agent streams model output, runs tools, waits for approvals, and accepts user input.

This document defines the high-level visual structure and responsive behavior. Specific composer behavior, streaming cell behavior, and terminal lifecycle cleanup are refined by adjacent TUI L2 documents.

## Source Requirements

- `L1-REQ-APP-007` requires an inline-capable terminal UI with header or status area, transcript area, composer area, onboarding, command discovery, and visible active work state.
- `L1-REQ-TUI-003` requires a readable and reviewable transcript.
- `L1-REQ-TUI-004` requires visible current execution state.
- `L1-REQ-TUI-007` requires responsive layout and readability.
- `L1-REQ-CLIENT-001` requires Unicode-safe and localization-ready client display behavior.
- `L1-REQ-CLIENT-002` requires active and restored sessions to render with consistent visual treatment.
- `L2-DES-APP-003` defines the canonical client/server events used to drive the UI.
- `L2-DES-CONV-001` defines transcript turns, items, and durable replay state.
- `L2-DES-TUI-003` defines composer and input-mode behavior.
- `L2-DES-TUI-004` defines streaming transcript and state rendering.
- `L2-DES-TUI-005` defines terminal lifecycle safety.
- `L2-DES-TUI-006` defines the full transcript alternate-screen overlay entered by `Ctrl+T`.
- `L2-DES-TUI-007` defines the shared live/replay transcript projection used by TUI renderers.
- `L2-DES-TUI-008` defines the shared TUI style system for tokens, symbols, spacing, and animation.

## Design Requirement

The TUI should be organized as a stable vertical shell with five conceptual regions:

1. Session header.
2. Transcript viewport.
3. Active work strip or working indicator.
4. Composer.
5. Bottom status line.

An optional pinned plan surface may appear between the transcript viewport and the composer when the active plan has pending, in-progress, or blocked work. It is a state surface owned by transcript/state rendering, not a separate shell mode.

Inline mode and alternate-screen mode should use the same conceptual shell. Inline mode additionally preserves useful terminal scrollback above the live region where possible.

## Modern TUI Principles

- Prioritize task state, transcript content, and composer usability over decoration.
- Keep region boundaries visually clear but lightweight.
- Use stable row allocation so streaming updates do not cause avoidable layout jitter.
- Use icons or compact labels only when they improve scanning.
- Avoid nested boxed panels. Use rows, separators, indentation, and compact cells instead.
- Treat color as secondary information. Text labels must still communicate state without color.
- Prefer summary lines plus expandable or scrollable detail for long content.
- Keep the composer visible whenever the terminal is large enough for interaction.

## Style System Boundary

The shell layout owns region order, sizing, clipping, and responsive priority. It must not hardcode color palettes, spinner frames, popup row styling, or status glyph semantics. Those choices are owned by `L2-DES-TUI-008`.

The shell should consume these style-system concepts:

- Header border, wordmark, label, and value tokens.
- Transcript and composer marker glyph `┃`.
- User/composer background-band tokens.
- Working spinner frames and active-state colors.
- Bottom status line separators, mode colors, and context progress glyphs.
- Responsive spacing rules for major regions.

## Standard Layout

The following sketch is normative for region order and relative priority. It is not a final choice of border glyphs, color, or exact column widths.

```text

┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ ██████╗  ███████╗██╗   ██╗ ██████╗                         ┃
┃ ██╔══██╗ ██╔════╝██║   ██║██╔═══██╗                        ┃
┃ ██║  ██║ █████╗  ██║   ██║██║   ██║                  v0.1.9┃
┃ ██║  ██║ ██╔══╝  ╚██╗ ██╔╝██║   ██║                        ┃
┃ ██████╔╝ ███████╗ ╚████╔╝ ╚██████╔╝                        ┃
┃ ╚═════╝  ╚══════╝  ╚═══╝   ╚═════╝                         ┃
┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫
┃ Model      deepseek-v4-pro                Reasoning   high ┃
┃ Workspace  ~/Desktop/devo                                  ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛

  Tip: Ready in /Users/username/Desktop/devo

┃ Fix the parser regression and run the focused tests.

┃ Thought: The failure is isolated to escaped quote handling.

┃ I will update the parser branch, add a regression test, and run the
  focused suite.

┃ Explore
  ┗ Read  crates/parser/src/lib.rs
    Grep  "quoted_escape" crates/parser tests

┃ Edit crates/parser/src/lib.rs

  @@ parse_value
  -        return parse_bare_value(input);
  +        return parse_quoted_or_bare_value(input);

⠋ Working · ⏱ 12s

┃ Ask Devo

  Build · deepseek-v4-pro high  ↑0[cached 0 0%]  ↓0  ▱▱▱▱▱▱▱▱▱▱  0%  0/950k
```

Required characteristics:

- The startup header provides product identity, version, current model, workspace, and reasoning effort.
- The transcript viewport owns most vertical space.
- The optional pinned plan surface appears only while there is active visible plan state.
- The active work strip appears only when useful and shows transient live state such as `⠋ Working · ⏱ 12s`.
- The composer remains above the bottom status line.
- The bottom status line is reserved for active mode, current model/reasoning, token usage, and context-window pressure.
- The `┃` glyph in transcript and composer regions is normally a single leading marker for the active prompt line, cell title, or first content line. User-message cells and the bottom input composer are background-band surfaces: when they contain multiple user-entered lines, each content line may repeat `┃`, while top and bottom padding rows keep the shared background without rendering the marker. Diff detail, tool output detail, and assistant wrapped text align under their content column and do not repeat the marker unless they are separate logical cells.

## Startup Header Visual Rules

The startup header should use the boxed ASCII layout from the standard sketch. It should be a first-screen identity and environment summary, not a repeating transcript cell.

Theme mapping:

- The ASCII-art product wordmark uses the theme primary foreground color.
- Header border glyphs such as `┏`, `━`, `┣`, `┃`, and `┗` use a muted grey foreground.
- Header metadata labels `Model`, `Workspace`, and `Reasoning` use muted grey.
- Header version text such as `v0.1.9` uses muted grey.
- Header metadata values, such as model display name, workspace path, and reasoning value, use normal white foreground. The model slug is an identifier and should appear only where disambiguation is needed.
- The header box should not rely on color alone; the border, labels, and values must remain readable in monochrome terminals.

The tip line below the header should use bold styling for `Tip:` and normal styling for the tip content.

```text
  Tip: Ready in /Users/username/Desktop/devo
```

In the rendered TUI, only `Tip:` is bold.

## Transcript Viewport In The Shell

The transcript viewport sits between the startup/session header and the bottom composer region. It is a scrollable list of transcript cells defined by `L2-DES-TUI-004`.

Shell-level responsibilities:

- Allocate most vertical space to the transcript viewport.
- Keep the single transcript cell marker column aligned with the composer marker column where practical.
- Preserve enough bottom space for the working indicator, composer, and status line.
- Let completed transcript cells scroll away while live composer and status regions remain fixed.
- Avoid embedding the transcript viewport inside a decorative card or nested box.

The shell owns placement and clipping. `L2-DES-TUI-004` owns the detailed rendering of user, assistant, tool, shell, working, and completed-turn cells.

The shell must not choose different layout or rendering paths based on whether a session is live or restored. It places the transcript projection defined by `L2-DES-TUI-007`; inline mode and restored-session mode differ only by transient overlays such as the composer, cursor, working spinner, and active animation state.

Example shell composition with transcript content:

```text
┃ Add escaping support for quoted parser values.

┃ Thinking: Checking existing parser tests and the quoted branch.

┃ The tests show escaped quotes are currently treated as ordinary text.

┃ Explore
  ┗ Read  crates/parser/src/lib.rs
    Read  tests/parser/quoted.rs

┃ Running  cargo test parser::quoted -- --nocapture

⠋ Working · ⏱ 8s

┃ Ask Devo

  Build · deepseek-v4-pro high  ↑420[cached 300 71%]  ↓12  ▰▰▱▱▱▱▱▱▱▱  20%  190k/950k
```

## Region Responsibilities

| Region | Responsibility | Must Avoid |
|---|---|---|
| Header | Startup identity, version, model, workspace, and reasoning effort. | Consuming too many rows or repeating full configuration. |
| Transcript viewport | Durable user-visible conversation, tool, approval, question, and error history. | Showing unlimited raw output inline. |
| Pinned plan surface | Active visible plan state when configured as pinned. | Becoming a second transcript or showing completed-only plans forever. |
| Active work strip | Current live work summary, waiting reason, running background process summary. | Becoming the only place important state appears. |
| Composer | Current editable input, popups, mode-specific input affordances. | Being pushed off-screen during streaming. |
| Bottom status line | Current mode, model/reasoning, token/cache usage, and context-window usage. | Duplicating long transcript content. |

## Responsive Rules

- The composer must remain usable at every supported terminal size.
- Long lines should wrap, fold, or truncate by display columns, not bytes.
- Folding or truncation must be visible when important content is omitted.
- Terminal resize should recompute layout from current state rather than incrementally shifting old rows.
- Optional metadata should collapse before transcript or composer content.
- Popups should be constrained to the visible terminal and should not cover the composer unless the popup is directly editing composer state.
- If the terminal is too small for meaningful interaction, the TUI should show a concise minimum-size message and preserve terminal safety.

## State Sources

The TUI shell should render from server-confirmed state whenever possible:

- Session snapshots from `session/open` or `session/subscribe`.
- Turn-scoped notifications such as `turn/started`, `turn/completed`, `item/started`, and `item/agentMessage/delta`.
- Usage updates from `turn/usage/updated`.
- Context pressure from session compaction notifications and context projection updates.
- Tool and background process events from item lifecycle and item delta notifications.
- Error diagnostics from `turn/failed` or session error projections.

Clients may optimistically render local input, but canonical state comes from the server protocol.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-007 | 1 | specs/L1/L1-REQ-APP-007-tui.md | Defines the high-level terminal shell, core regions, inline/fullscreen consistency, and visible active-work layout. |
| refines | L1-REQ-TUI-007 | 1 | specs/L1/L1-REQ-TUI-007-responsive-layout-readability.md | Defines responsive priorities, narrow layout behavior, and non-overlap rules. |
| related-to | L1-REQ-TUI-003 | 1 | specs/L1/L1-REQ-TUI-003-transcript.md | Provides the transcript viewport placement and layout constraints. |
| related-to | L1-REQ-TUI-004 | 1 | specs/L1/L1-REQ-TUI-004-state-visibility.md | Defines shell regions that expose execution state. |
| related-to | L1-REQ-CLIENT-001 | 1 | specs/L1/L1-REQ-CLIENT-001-localization-readiness.md | Responsive layout must account for Unicode and localized display width. |
| related-to | L1-REQ-CLIENT-002 | 1 | specs/L1/L1-REQ-CLIENT-002-session-rendering-consistency.md | The shell places the same transcript projection for live and restored sessions. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Protocol events provide canonical state for the shell. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Durable transcript records are rendered in the viewport. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Composer and input mode behavior fills the shell's bottom regions. |
| related-to | L2-DES-TUI-004 | 1 | specs/L2/tui/L2-DES-TUI-004-streaming-transcript-and-state.md | Streaming transcript cells and state indicators populate the shell. |
| related-to | L2-DES-TUI-005 | 1 | specs/L2/tui/L2-DES-TUI-005-terminal-lifecycle-safety.md | Terminal lifecycle behavior constrains inline and alternate-screen shell modes. |
| related-to | L2-DES-TUI-006 | 1 | specs/L2/tui/L2-DES-TUI-006-full-transcript-alternate-screen.md | Defines the alternate-screen transcript review surface entered from the inline shell. |
| related-to | L2-DES-TUI-007 | 1 | specs/L2/tui/L2-DES-TUI-007-session-rendering-consistency.md | Defines the shared live/replay projection that the shell places in the transcript viewport. |
| related-to | L2-DES-TUI-008 | 1 | specs/L2/tui/L2-DES-TUI-008-style.md | Defines the shell's shared color, symbol, spacing, and animation style rules. |
| specified-by | L3-BEH-TUI-001 | 2 | specs/L3/tui/L3-BEH-TUI-001-layout-composer-input.md | L3 defines the shell regions, responsive resize behavior, composer band, working indicator, and status line. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-23 | Assistant | Initial | Initial modern TUI shell and responsive layout design. |
| 1 | 2026-05-23 | Human | Refinement | Added concrete startup header sketch, theme-color rules, bold tip label, composer prompt band, and token/context status line shape. |
| 1 | 2026-05-23 | Human | Refinement | Clarified that `┃` is a single leading marker, not a rail repeated through the full cell. |
| 1 | 2026-05-23 | Human | Refinement | Updated working indicator examples to use the spinner frame style and kept consecutive read calls on separate Explore lines. |
| 1 | 2026-05-23 | Human | Refinement | Clarified that multi-line composer and user-message background bands may repeat `┃` on content lines, but not on padding rows. |
| 1 | 2026-05-25 | Assistant | Refinement | Linked the shell layout to the `Ctrl+T` full transcript alternate-screen design. |
| 1 | 2026-05-25 | Assistant | Refinement | Linked shell placement to the shared live/replay transcript projection in `L2-DES-TUI-007`. |
| 2 | 2026-05-26 | Assistant | Revision | Linked the shell layout to the shared TUI style system and clarified that visual tokens, symbols, spinner frames, and spacing are owned by `L2-DES-TUI-008`. |
| 3 | 2026-05-26 | Assistant | Revision | Added the optional pinned plan surface as a shell placement between transcript and composer. |
