---
artifact_id: L2-DES-TUI-008
revision: 4
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-26
---

# L2-DES-TUI-008 — TUI Style System

## Purpose

Define the visual style system for the TUI: color tokens, symbols, spacing, animation, component styling, and accessibility rules used by the shell, composer, transcript, slash-command flows, and full transcript review.

## Background / Context

The TUI should feel like a polished terminal-native work surface: compact, readable, responsive, and visually rich without becoming decorative noise. Existing TUI design documents define layout and behavior. This document defines the shared visual grammar those documents should use.

The style system must be product-owned. It should not depend on another application's theme names, renderer internals, or copied implementation symbols. Devo may learn from mature terminal interfaces, but the resulting design must be expressed as Devo configuration, Devo theme tokens, and Devo component contracts.

## Source Requirements

- `L1-REQ-APP-007` requires a usable terminal UI with header, transcript, composer, command discovery, onboarding, and visible active state.
- `L1-REQ-TUI-001` requires reliable composer behavior and command discovery.
- `L1-REQ-TUI-002` requires timely streaming display.
- `L1-REQ-TUI-003` requires a readable and reviewable transcript.
- `L1-REQ-TUI-004` requires visible execution state.
- `L1-REQ-TUI-007` requires responsive layout and readability.
- `L1-REQ-CLIENT-001` requires Unicode-safe and localization-ready rendering.
- `L1-REQ-CLIENT-002` requires live and restored sessions to share a consistent rendering pipeline.
- `L2-DES-TUI-002` defines the shell layout.
- `L2-DES-TUI-003` defines composer and input modes.
- `L2-DES-TUI-004` defines streaming transcript cells and state.
- `L2-DES-TUI-005` defines terminal lifecycle safety.
- `L2-DES-TUI-006` defines full transcript alternate-screen review.
- `L2-DES-TUI-007` defines live/replay rendering consistency.
- `L2-DES-APP-005` defines the durable `config.toml` schema that stores TUI preferences.
- `L2-DES-CLIENT-001` defines Unicode and localization constraints.

## Design Requirement

The TUI should render through semantic style tokens rather than hardcoded colors or ad hoc glyph choices. All major visible surfaces should derive their colors, symbols, spacing, and state indicators from this style system.

The style system must:

1. Define a small set of semantic color tokens with dark and light theme values.
2. Define canonical symbols for borders, transcript markers, status, relationships, navigation, progress, and animation.
3. Keep layout density high enough for engineering work while preserving clear visual grouping.
4. Make color secondary to text and symbol shape, so monochrome or low-color terminals remain usable.
5. Preserve Unicode, CJK, IME, and display-width correctness.
6. Support optional enhanced glyphs without requiring Nerd Font or private-use glyphs.
7. Apply the same visual grammar to live and restored session rendering.

## Visual Direction

The TUI should feel:

- **Terminal-native**: designed for monospace cells, not a web page squeezed into a terminal.
- **Dense but calm**: enough information is visible for repeated work, with quiet hierarchy and restrained separators.
- **Action-oriented**: active work, warnings, approvals, errors, and next actions are easy to scan.
- **Consistent**: the same glyph and color should mean the same thing everywhere.
- **Professional**: no decorative noise, oversized boxed areas, or inconsistent status styling.

The preferred default visual identity is a dark terminal theme with a warm orange primary accent, neutral grey structure, near-white content text, and secondary accents for success, warning, error, shell, plan, and informational states.

## Rendering Model

This L2 design does not require a specific TUI framework. Implementations may use any renderer that can satisfy these constraints:

- Terminal-cell layout with display-width-aware measurement.
- Stable vertical region allocation for header, transcript, working indicator, composer, and status line.
- Styled text spans with foreground, background, bold, dim, and italic where supported.
- Incremental repaint for streaming content.
- Alternate-screen entry and restoration for full transcript review.
- Theme token lookup from durable configuration and runtime overrides.

Model output must not be allowed to render arbitrary UI components directly. If future structured rich blocks are supported, they must be parsed through an allowlisted, program-owned schema and rendered with the tokens in this document.

## Theme Tokens

Themes should expose semantic tokens. Component code should request tokens by role, not by hex color.

### Devo Dark

```yaml
name: "devo-dark"
appearance: "dark"

core:
  primary: "#d75f00"
  border: "#878787"
  borderMuted: "#4e4e4e"
  success: "#afaf5f"
  error: "#d75f5f"
  warning: "#ffaf00"
  info: "#5fafd7"
  plan: "#afafff"
  shell: "#5f8787"
  btw: "#8fb9c9"
  focus: "#d78700"

text:
  primary: "#eeeeee"
  secondary: "#a8a8a8"
  muted: "#767676"
  disabled: "#5f5f5f"
  inverse: "#000000"
  link: "#af87af"

surface:
  terminal: "#1c1c1c"
  userBand: "#262626"
  inputBand: "#262626"
  selection: "#3a3a3a"
  popup: "#1c1c1c"
  popupBorder: "#4e4e4e"

mode:
  build: "#d75f00"
  plan: "#afafff"
  shell: "#5f8787"

tool:
  name: "#d7875f"
  parameter: "#b2b2b2"
  badgeBackground: "#FEB17F"
  badgeForeground: "#000000"
  active: "#ff5f00"
  done: "#afaf5f"
  failed: "#af5f5f"

diff:
  added: "#5fff5f"
  addedBackground: "#0a2e10"
  removed: "#ff5f5f"
  removedBackground: "#2e0a0a"
  metadata: "#5fafd7"
  lineNumber: "#767676"
  unchanged: "#767676"
  border: "#585858"

markdown:
  bold: "#bcbcbc"
  italic: "#bcbcbc"
  code: "#8a8a8a"
  heading: "#dadada"
  blockquote: "#8a8a8a"
  strikethrough: "#8a8a8a"

terminal:
  background: "#1c1c1c"
  foreground: "#eeeeee"
  cursor: "#d75f00"
```

### Devo Light

```yaml
name: "devo-light"
appearance: "light"

core:
  primary: "#d75f00"
  border: "#6b6b6b"
  borderMuted: "#c8c8c8"
  success: "#2e7d32"
  error: "#c62828"
  warning: "#e65100"
  info: "#0057a8"
  plan: "#5b5fc7"
  shell: "#00695c"
  btw: "#006d8f"
  focus: "#b86200"

text:
  primary: "#1a1a1a"
  secondary: "#4a4a4a"
  muted: "#6b6b6b"
  disabled: "#9a9a9a"
  inverse: "#ffffff"
  link: "#7b1fa2"

surface:
  terminal: "#f5f5f5"
  userBand: "#e8e8e8"
  inputBand: "#e8e8e8"
  selection: "#dedede"
  popup: "#f5f5f5"
  popupBorder: "#c8c8c8"

mode:
  build: "#d75f00"
  plan: "#5b5fc7"
  shell: "#00695c"

tool:
  name: "#9a4f00"
  parameter: "#4a4a4a"
  badgeBackground: "#ffd0a8"
  badgeForeground: "#1a1a1a"
  active: "#d75f00"
  done: "#2e7d32"
  failed: "#c62828"

diff:
  added: "#0b6b18"
  addedBackground: "#d8f0dc"
  removed: "#9f1d1d"
  removedBackground: "#f2d6d6"
  metadata: "#0057a8"
  lineNumber: "#6b6b6b"
  unchanged: "#6b6b6b"
  border: "#c8c8c8"

markdown:
  bold: "#1a1a1a"
  italic: "#4a4a4a"
  code: "#4a4a4a"
  heading: "#1a1a1a"
  blockquote: "#6b6b6b"
  strikethrough: "#6b6b6b"

terminal:
  background: "#f5f5f5"
  foreground: "#1a1a1a"
  cursor: "#d75f00"
```

## Theme Management

The default built-in themes are `devo-dark` and `devo-light`. `devo-dark` should be the default unless user or workspace configuration selects another theme.

Theme selection is user-facing through `/theme` and durable through the `[tui]` section of `config.toml`.

Rules:

- Theme lookup resolves a named theme to a complete token set before rendering.
- User-installed themes may be supported by L3 design, but they must resolve to the semantic token names in this document.
- A theme may extend a built-in theme if the implementation can validate the resolved result.
- Missing required tokens fail closed to a known built-in theme and produce a concise diagnostic.
- Terminal foreground, background, and cursor color override is optional and opt-in.
- Any terminal color override must be restored on shutdown, interrupt, and alternate-screen exit according to `L2-DES-TUI-005`.
- Runtime theme switching must repaint the current frame without mutating transcript content or session state.

## Symbol System

Symbols should be used deliberately. Each symbol family has a canonical purpose.

| Symbol Family | Symbols | Purpose |
|---|---|---|
| Header box | `┏ ┓ ┗ ┛ ┣ ┫ ━ ┃` | Startup header identity box only. |
| Light structure | `─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼` | Tables, subtle separators, and compact internal structure. |
| Transcript marker | `┃` | Single leading marker for composer, user messages, assistant cells, and tool cells. |
| Relationship marker | `┗` | Connect tool titles to output summaries or grouped child rows. |
| Progress | `▰ ▱ █` | Context usage, progress bars, filled portions. |
| Choice focus | `>` | Keyboard-highlighted row in a navigable list. |
| Choice enabled | `●` | Currently enabled or active option in a navigable single-choice list. |
| Disclosure | `▶ ▼` | Collapsed and expanded sections. |
| Status | `✓ ✗ ⚠ ℹ` | Success, failure, warning, information. |
| Navigation | `↑ ↓ → ← ↩` | Key hints and direction, never as decoration. |
| Completion | `▣` | Completed assistant turn summary. |

Rules:

- `┃` is a marker, not a full-height rail. It appears on the first line of a logical cell or on each user-authored content line inside user/composer bands.
- Header double-line glyphs are reserved for the startup/session header so they keep visual weight.
- Avoid emoji for core state because terminal width and glyph availability vary. Prefer the symbols above.
- Use `…` for truncation only when content is actually omitted.
- Use `·` as the inline separator in status lines.
- In live active-work indicators, elapsed time uses the timer symbol before the compact duration, such as `⠋ Working · ⏱ 12s`.
- In navigable TUI-style lists, `>` and `●` are independent marker columns. `>` marks the currently focused row controlled by Up and Down. `●` marks the option that is already enabled or currently active.
- If the focused row is also the enabled row, render both markers, such as `> ● high`. If the focused row is not enabled, render `>   medium`. If a non-focused row is enabled, render `  ● high`.
- Do not use `●` merely to mean hovered, focused, or highlighted. Outside navigable choice lists, `●` may still be used by domain-specific components such as Plan cells where the component defines a separate status meaning.

## Spinner And Animation

The standard spinner sequence is:

```text
⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏
```

Rules:

- Use a stable frame interval around 80-120ms.
- Use the theme primary color for ordinary active work.
- Use warning or error colors only when the state itself is warning or failed.
- Animation is live-only. Restored transcript history must render a static completed, failed, interrupted, or canceled state.
- The same spinner is used for thinking, tool execution, compaction, streaming assistant output, and startup loading; the label provides the specific state.

Examples:

```text
⠋ Working · ⏱ 12s
⠼ Compacting · ⏱ 4s
⠦ Starting server · ⏱ 8s
```

## Spacing And Layout

Spacing should use terminal-cell units.

| Spacing | Use |
|---|---|
| `0` | Dense inner rows and status lines. |
| `1` | Default row gap, popup padding, compact panel interior. |
| `2` | Rare emphasis around major flows such as onboarding sections. |

Rules:

- Prefer one blank row between major transcript cells only when it improves scanability.
- Do not insert blank rows between different result types inside a popup list.
- User-message and composer bands have one padding row above and below content.
- Tool output details align under the content column, not under the marker column.
- Long output folds before it pushes the composer out of view.

## Component Style Contracts

### Header

- Wordmark uses `core.primary`.
- Header border uses `core.border`.
- Labels such as `Model`, `Workspace`, and `Reasoning` use `text.muted`.
- Values use `text.primary`.
- Version text uses `text.muted`.
- The header is shown on startup and session summary surfaces, not repeated as a transcript cell.

### User Message Band

- Background uses `surface.userBand`.
- `┃` uses `core.primary`.
- Text uses `text.primary`.
- Top and bottom padding rows share the background and do not render `┃`.
- Multi-line user-authored content may repeat `┃` on each content line.

### Composer Band

- Background uses `surface.inputBand`.
- Empty hint `Ask Devo` uses `text.muted`.
- User input uses `text.primary`.
- Matched slash command token uses `core.primary`.
- Slash command parameter hint uses `text.muted`.
- Build, Plan, and Shell status labels use `mode.build`, `mode.plan`, and `mode.shell`.

### Assistant Cells

- Assistant reply text uses `text.primary`.
- Reasoning body uses `text.muted`.
- `Thinking:` and `Thought:` labels use an accent-muted token and italic style where supported.
- Wrapped continuation lines align under the content column and do not repeat `┃`.
- Completed-turn summary uses `▣` in `core.primary`, followed by mode, model display name, and elapsed time.

### Tool Cells

- Tool title marker `┃` uses `tool.name` or `core.primary` depending on the tool family.
- Tool parameters and paths use `tool.parameter`.
- Status uses `tool.active`, `tool.done`, or `tool.failed`.
- Relationship rows use `┗` in `core.border`.
- Long output is folded with visible hidden-line counts.
- Diff blocks use `diff.*` tokens and remain readable without color through `+`, `-`, and metadata prefixes.

### Popups And Search Lists

- Popups do not display the word "popup" in their own content.
- Search hint or field purpose appears at the top only when it helps the user identify the current field.
- Navigable choice rows should reserve marker columns before the label using `<focus><space><enabled><space>`, where focus is `>` or space and enabled is `●` or space.
- Pure search-result lists that have no enabled/current option may omit the enabled marker column, but must still use `>` for the focused row when keyboard navigation is active.
- Markerless rows keep two-character left padding.
- The focused row uses `core.primary` for both name and description.
- Non-focused row names use `text.primary`; non-focused descriptions use `text.muted`.
- A non-focused enabled row keeps normal label color but shows the `●` marker in `core.primary`.
- Different result types should not be separated by blank rows unless the list is truly segmented by a required heading.
- The popup must fit inside the visible terminal and must not hide the composer unless it is directly attached to composer input.

### Approval And Question Surfaces

- Pending approval uses `core.warning`.
- Approved state uses `core.success`.
- Denied or failed state uses `core.error`.
- Options must remain understandable without color.
- Key hints use muted text and a consistent pattern such as `↑/↓ navigate · Enter select · Esc cancel`.

### Plan Cells

- Pending uses `○`.
- In progress uses `●` with `core.primary`.
- Completed uses `✓` with `core.success`.
- Blocked uses `⚠` with `core.warning`.
- Nested plan items may use light box-drawing connectors, but should remain compact.
- The plan header uses `Plan · completed/total` with `Plan` in `text.secondary` or `core.primary` depending on emphasis.
- The pinned plan surface uses only a left structural marker or border; it should not be surrounded by a full box.
- Completed plan text may use strikethrough where supported and muted foreground.
- Plan overflow uses muted truncation text such as `… 3 more, Ctrl+T to view all`.
- Detailed Plan cell behavior and transcript examples are specified by `L2-DES-TUI-004`.

Style example:

```text
  Plan · 1/4
┃ ✓ Inspect existing parser branch
┃ ● Patch quoted value parsing
┃ ○ Add escaped quote regression
┃ ○ Run focused parser tests
```

## Rich Terminal Widgets

Program-owned UI surfaces may use richer terminal widgets when they communicate state better than prose.

Allowed widget families:

- `Badge`: short inline label for state or mode.
- `ProgressBar`: context usage, task progress, or operation progress.
- `KeyValue`: compact session/config/status rows.
- `Table`: aligned data where columns fit; degrade to key-value rows on narrow terminals.
- `List`: simple option or summary lists.
- `Metric`: token, cache, duration, or count display.
- `Callout`: warning, error, or important instruction.
- `Timeline`: ordered lifecycle events such as compaction or background process history.

Rules:

- These widgets are renderer-owned components, not arbitrary model-supplied UI.
- They must degrade to plain text in narrow terminals.
- They must use semantic theme tokens.
- They should not appear inside normal assistant prose unless the server intentionally projects a structured item as a widget.

## Accessibility And Terminal Compatibility

- Nerd Font and private-use glyphs are optional enhancements only.
- Every private-use or advanced icon must have a plain Unicode or text fallback.
- All text measurement must use grapheme clusters and display columns, not bytes.
- CJK, combining marks, emoji-width variance, and IME composition must not corrupt layout.
- Color must not be the only carrier of meaning.
- Theme contrast should remain readable in both dark and light modes.
- Terminal color override through OSC escape sequences may be supported, but must be opt-in and must restore colors on shutdown through `L2-DES-TUI-005`.
- Sound is out of scope for the current TUI style baseline.

## Example Polished Frame

```text
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ ██████╗  ███████╗██╗   ██╗ ██████╗                  v0.1.9┃
┃ ██╔══██╗ ██╔════╝██║   ██║██╔═══██╗                       ┃
┃ ██║  ██║ █████╗  ██║   ██║██║   ██║                       ┃
┃ ██████╔╝ ███████╗ ╚████╔╝ ╚██████╔╝                       ┃
┃ ╚═════╝  ╚══════╝  ╚═══╝   ╚═════╝                        ┃
┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫
┃ Model      deepseek-v4-pro                Reasoning   high ┃
┃ Workspace  ~/Desktop/devo                                  ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛

  Tip: Ready in /Users/username/Desktop/devo

┃ Refactor the parser in three steps:
┃ 1. isolate quoted-value parsing
┃ 2. add regression tests
┃ 3. run the focused suite

┃ Thought: The failure is isolated to escaped quote handling.

┃ Explore
  ┗ Read  crates/parser/src/lib.rs
    Grep  "quoted_escape" crates/parser tests

┃ Edit crates/parser/src/lib.rs

  @@ parse_value
  -        return parse_bare_value(input);
  +        return parse_quoted_or_bare_value(input);

⠋ Working · ⏱ 12s

┃ Ask Devo

  Build · deepseek-v4-pro high  ↑420[cached 300 71%]  ↓12  ▰▰▱▱▱▱▱▱▱▱  20%  190k/950k
```

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TUI-007 | 1 | specs/L1/L1-REQ-TUI-007-responsive-layout-readability.md | Defines style, spacing, symbol, and theme rules required for readable terminal layouts. |
| related-to | L1-REQ-APP-007 | 1 | specs/L1/L1-REQ-APP-007-tui.md | Provides the visual system for the terminal UI required by the app-level TUI requirement. |
| related-to | L1-REQ-TUI-001 | 1 | specs/L1/L1-REQ-TUI-001-composer.md | Defines composer band styling, slash-command color rules, and input surface tokens. |
| related-to | L1-REQ-TUI-002 | 1 | specs/L1/L1-REQ-TUI-002-streaming.md | Defines live spinner, streaming cell, and active state style rules. |
| related-to | L1-REQ-TUI-003 | 1 | specs/L1/L1-REQ-TUI-003-transcript.md | Defines transcript markers, cell styling, folded output, and reviewable visual grammar. |
| related-to | L1-REQ-TUI-004 | 1 | specs/L1/L1-REQ-TUI-004-state-visibility.md | Defines state colors, status symbols, approval styling, and active work indicators. |
| related-to | L1-REQ-CLIENT-001 | 1 | specs/L1/L1-REQ-CLIENT-001-localization-readiness.md | Style and symbol choices must remain Unicode-safe and localization-ready. |
| related-to | L1-REQ-CLIENT-002 | 1 | specs/L1/L1-REQ-CLIENT-002-session-rendering-consistency.md | Live and restored sessions must use the same style tokens and renderers. |
| related-to | L2-DES-TUI-002 | 1 | specs/L2/tui/L2-DES-TUI-002-modern-tui-shell-layout.md | The shell layout consumes header, region, spacing, and status tokens. |
| related-to | L2-DES-TUI-003 | 1 | specs/L2/tui/L2-DES-TUI-003-composer-and-input-modes.md | Composer and command discovery consume input band and popup style rules. |
| related-to | L2-DES-TUI-004 | 1 | specs/L2/tui/L2-DES-TUI-004-streaming-transcript-and-state.md | Transcript and streaming cells consume symbol, state, and tool style rules. |
| related-to | L2-DES-TUI-005 | 1 | specs/L2/tui/L2-DES-TUI-005-terminal-lifecycle-safety.md | Terminal color override and restoration are constrained by lifecycle safety. |
| related-to | L2-DES-TUI-006 | 1 | specs/L2/tui/L2-DES-TUI-006-full-transcript-alternate-screen.md | Full transcript review uses the same style tokens at expanded output limits. |
| related-to | L2-DES-TUI-007 | 1 | specs/L2/tui/L2-DES-TUI-007-session-rendering-consistency.md | Shared projections must render through the same style system. |
| related-to | L2-DES-APP-005 | 2 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | Theme selection and TUI preferences are persisted in `config.toml`. |
| related-to | L2-DES-CLIENT-001 | 1 | specs/L2/client/L2-DES-CLIENT-001-localization-readiness.md | Unicode display-width rules constrain all styled rendering. |
| specified-by | L3-BEH-TUI-007 | 1 | specs/L3/tui/L3-BEH-TUI-007-style-system.md | L3 defines theme tokens, symbols, spinner frames, popup markers, component styles, and style tests. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-26 | Assistant | Initial | Initial Devo TUI style system, replacing raw external design notes with semantic tokens, symbols, component style contracts, accessibility rules, and a polished example frame. |
| 2 | 2026-05-26 | Assistant | Minor | Added a direct Plan cell style example and linked detailed Plan behavior back to `L2-DES-TUI-004`. |
| 3 | 2026-05-26 | Human | Refinement | Defined navigable-list marker semantics: `>` for focused row and `●` for currently enabled option. |
| 4 | 2026-05-27 | Human | Refinement | Added the `⏱` symbol before elapsed time in live active-work indicators. |
