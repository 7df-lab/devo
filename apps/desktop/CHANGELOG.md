# @devo/desktop

## 0.11.0

### Minor Changes

- [`5dd20fd`](https://github.com/tsiao/devo/commit/5dd20fd310c05b2e8e16c25e472334a84714410c) - Sub-agent permissions and questions now bubble up to the parent session

  When Claude spawns a sub-agent via the Task tool, any interactive request that the sub-agent raises — a tool-use permission or a follow-up question — is now surfaced directly in the parent session's input area. This means you no longer need to navigate to the child session to unblock a running task.

  **What changed:**

  - **Parent chat input**: Shows the sub-agent's permission card or question flow with a "Sub-agent" indicator. Responding from the parent correctly targets the child session.
  - **Sidebar**: The parent session's status turns to "waiting" (amber dot) when any descendant sub-agent is blocked, making it visible at a glance.
  - **Sub-agent card**: While a sub-agent is running and waiting for input, its card header shows a "Needs approval" or "Asking a question" badge in amber.
  - **Notifications**: System notifications and the dock badge count are now raised for sub-agent permissions/questions too, attributed to the root parent session so clicking them takes you to the right place.
  - **Bug fix**: Permission and question responses were previously always routed to the parent session ID. They now correctly target the session that owns the request, even when that session is a sub-agent.

- [`11f14e1`](https://github.com/tsiao/devo/commit/11f14e1e6c55c535e7f12747958d45170140e117) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Cleaner top bar, responsive layout, and session UX improvements

  **Top bar cleanup:** Removed the stop button (redundant — chat input is always visible), the status dot/label (redundant — visible from the chat itself), and the error/retry alert triangles from the metrics bar (alarming without context; errors are visible inline in the chat).

  **Responsive layout:** The chat view and new-chat screen now adapt to narrow viewports — padding collapses on small screens and the suggestion grid switches to a single column. The sidebar auto-collapses when the window drops below 600px and restores when it grows back (manual closes are respected).

  **Session task list:** Smoother expand/collapse via CSS grid-row height transition. Task items animate in with a staggered fade+slide and re-animate on status change. Visual weight reduced with softer colors and lighter borders.

  **Session switching:** No more loading spinner flash when switching back to a previously-visited session that has cached messages. The "Load earlier messages" button no longer bleeds through from one session to another on switch.

  **Window:** Removed the 900×600 minimum window size constraint.

### Patch Changes

- [`735fc2f`](https://github.com/tsiao/devo/commit/735fc2f9a3ee8ff0e8a1cd206bd4c6cd6a9fc0d4) - Fix stale sessions on server switch and amber banners invisible on light theme

  **Stale sessions (#57):** When switching between local and VPS servers, the old server's
  sessions could reappear in the sidebar after being cleared. Two root causes fixed:

  1. The SSE event batcher's `dispose()` was flushing buffered events into the store even
     after `disconnect()` was called, re-adding stale session IDs that had just been cleared
     by `triggerServerSwitch()`. The batcher now discards pending events when the connection
     is stale instead of flushing them.

  2. Per-project pagination state (`projectPaginationFamily`) was never reset on server
     switch. This caused expanded project directories to show as "loaded" on the new server,
     preventing a fresh session fetch. Pagination is now reset for all known directories
     during `triggerServerSwitch()`.

  **Banner visibility (#61):** The automations permissions banner and the chat session revert
  banner used hardcoded `text-amber-200` / `text-amber-300` shades which are near-white and
  invisible on light theme backgrounds. Both banners now use `dark:` variants so they render
  correctly on both light and dark themes.

## 0.10.0

### Minor Changes

- [`4af265c`](https://github.com/tsiao/devo/commit/4af265cad1b6538927a823d0f3d52c9537b17445) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Enable automations by default and add permissions info banner

  Automations are now enabled by default for all users instead of requiring opt-in via the command palette. A dismissible info banner appears at the top of the Automations tab explaining that automations run unattended with broad permissions (all tools allowed, interactive prompts auto-denied) and pointing users to the read-only preset to restrict access. The Beta badge has also been removed from the Automations toolbar.

- [`c3aa783`](https://github.com/tsiao/devo/commit/c3aa7832c2bff0352c167218c7f4d20da7129d6f) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Add review panel with diff viewer and inline diff comments

  A new slide-in review panel lets you inspect all file changes produced by the current session without leaving the chat. The panel shows a full diff viewer powered by a background worker pool for fast rendering, alongside a running list of changes grouped by file.

  Diff comments can be written directly from the review panel and are injected into the chat input — so you can ask the agent to revisit specific changes without manually copying file paths or line numbers. The panel slides in from the right and adapts the chat layout automatically so nothing is obscured.

- [`c3aa783`](https://github.com/tsiao/devo/commit/c3aa7832c2bff0352c167218c7f4d20da7129d6f) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Lazy session loading, pagination, and project search

  The sidebar now loads sessions lazily per project rather than fetching everything on startup. A "Load more" button in each project section pages through sessions in batches, dramatically reducing initial load time on large workspaces.

  Project search lets you filter the sidebar by name in real time. Session metrics (token counts, cost, duration) have moved into a compact popover on the session header to reduce visual clutter. Tool call durations now use a client-side first-seen timestamp for accuracy instead of relying solely on server-reported times.

### Patch Changes

- [`c3aa783`](https://github.com/tsiao/devo/commit/c3aa7832c2bff0352c167218c7f4d20da7129d6f) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Automation improvements: agent/model selection, minute-level schedules, and reliability

  Automation configurations now support selecting a specific agent, model, and model variant — giving you the same control over automated runs as you have over manual sessions.

  The schedule interval picker now accepts minutes in addition to hours and days, enabling sub-hourly automation schedules. The scheduler itself has been made async and now tracks next-run times in memory to avoid stale database reads. The automation executor has also been hardened with improved error logging and retry behaviour.

- [`c3aa783`](https://github.com/tsiao/devo/commit/c3aa7832c2bff0352c167218c7f4d20da7129d6f) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Chat UX improvements: elapsed time, @ mentions on new sessions, bash streaming, and more

  - Running tool calls and sub-agent cards now show a live elapsed time counter (e.g. "12s", "1m 4s") that ticks every second, so you always know how long a tool has been active without waiting for it to finish
  - @ file and agent mentions now work on the new session input screen — you no longer need to create a session first before tagging files
  - A **New Session** button has been added to the sidebar navigation for quick access from anywhere in the app
  - Bash tool cards stream live stdout/stderr output as it arrives during execution, matching the behaviour of the Devo TUI
  - Tool calls nested inside reasoning blocks are now grouped inside the collapsible reasoning section rather than appearing loose in the chat

- [`c3aa783`](https://github.com/tsiao/devo/commit/c3aa7832c2bff0352c167218c7f4d20da7129d6f) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Platform and infrastructure improvements

  - **Linux/Wayland**: resolved rendering issues that caused visual glitches on Wayland compositors
  - **Linux tray icon**: added a dedicated tray icon for Linux so the app integrates properly with system trays on GNOME, KDE, and compatible desktops
  - **mDNS discovery**: a new settings page lets you configure mDNS-based server discovery for finding Devo instances on your local network automatically
  - **Provider icons**: provider icons are now fetched at runtime from models.dev rather than being bundled, keeping them up to date as new providers are added
  - **Server lockfile**: the app now writes a lockfile when it owns the Devo server process, preventing multiple instances from fighting over the same server on startup

- [`c3aa783`](https://github.com/tsiao/devo/commit/c3aa7832c2bff0352c167218c7f4d20da7129d6f) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Bug fixes

  - **Subagent sessions not found on VPS**: navigating to a subagent session that isn't in the local store (e.g. after a reconnect or when the initial batch load excluded it) now falls back to a direct server fetch instead of showing a dead "not found" screen
  - **@ mention on new sessions**: fixed a silent failure where typing `@` on the new session screen showed nothing because the file search had no directory to query against
  - **Slash command race condition**: fixed an issue where selecting a slash command from the popover sometimes sent stale text (e.g. `/un` instead of `/undo`) due to a `setText + setTimeout` race with React's async batching. Popover keyboard delegation also no longer drops the first keypress due to a stale closure on `slashOpen`/`mentionOpen`
  - **OAuth provider loop**: fixed an infinite authorize loop and missing device code display for OAuth-based providers — thanks to [@YoruAkio](https://github.com/YoruAkio) for the fix!
  - **Native module packaging**: switched to the hoisted linker to fix packaging failures for native Node modules on all platforms
  - **Session and turn error deduplication**: error messages that appeared multiple times in the chat due to duplicate SSE events are now deduplicated
  - **Worktree settings API calls**: fixed excessive API requests being fired on the worktree settings page when no worktree was active
  - **Sidebar project sort order**: project order in the sidebar no longer jumps around between renders
  - **Session title width**: prevented the session title in the app bar from stretching to full width on wide windows
  - **Horizontal overflow**: resolved a layout issue causing content to overflow horizontally at narrow window widths

## 0.9.0

### Minor Changes

- [`4eb3d38`](https://github.com/tsiao/devo/commit/4eb3d387c8581ed230fb47f6432505afbfa66f41) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Redesign default chat display mode with grouped tool summaries

  The default view now renders an interleaved stream of text, reasoning blocks, and grouped tool summaries instead of a pill-bar summary. Consecutive tool calls of the same category (explore, edit, run, etc.) are collapsed into a single inline chip (e.g. "Read 3 files", "Edited foo.tsx, bar.tsx") with a left-border color accent. Each group chip is clickable and expands inline to show the full tool cards for that group. Groups with only a single tool skip the summary row and render the full tool card directly. A "Show N steps" toggle reveals all tool cards at once in verbose style.

### Patch Changes

- [`6cfc6ce`](https://github.com/tsiao/devo/commit/6cfc6ceba6401d51580935baf2d15605550839fc) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix automation permission ruleset and rrule ESM interop

  Automations were sometimes blocked because the default permission preset didn't include an explicit allow-all rule before the interactive-prompt denies. The ruleset now starts with `{ permission: "*", pattern: "*", action: "allow" }` so all tool calls pass through unless explicitly denied.

  Also fixes a CJS/ESM interop issue with the `rrule` package in the main process and renderer: `RRule` is now resolved via `rruleModule.RRule ?? rruleModule.default?.RRule` so it works in both build contexts.

- [`998f8cb`](https://github.com/tsiao/devo/commit/998f8cbf857568dc93c284c8a72a5c56ba3457f4) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Remove compact display mode

  The "Compact" display mode has been removed. The display mode type is now `"default" | "verbose"` only. Existing users with `compact` persisted in localStorage are automatically migrated to `default` on next launch.

## 0.8.0

### Minor Changes

- [`23a9317`](https://github.com/tsiao/devo/commit/23a931701e0b9a27d29a5962f4c9a28880a3a0a5) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Add session forking to branch conversations from any point. Fork from per-turn hover actions, sidebar context menu, `/fork` slash command, or the command palette. Per-turn fork cuts at the turn boundary; other entry points copy the entire conversation.

### Patch Changes

- [#40](https://github.com/tsiao/devo/pull/40) [`1393d87`](https://github.com/tsiao/devo/commit/1393d8758b0f7656cb6fd98728d883f9f6fdc389) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix duplicate error messages in chat view when the server emits both a session-level error and an assistant message error for the same failure. Thanks [@YoruAkio](https://github.com/YoruAkio) for the contribution!

- [#38](https://github.com/tsiao/devo/pull/38) [`566455e`](https://github.com/tsiao/devo/commit/566455ecbd5b3f882e5c66a4c8add250006c39e1) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix native module packaging by switching Bun to hoisted installs, resolving the `Could not find module '@libsql/darwin-x64'` crash on macOS x64 builds

- [`5f353b3`](https://github.com/tsiao/devo/commit/5f353b3ca4c8a8a987e9489932a4d5ebe5483aa3) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix unstable sidebar project sort order caused by volatile server-side `project.time.updated` timestamps. Projects now use a tiered sort: active agents first (by recency), idle sessions next (by recency), then sessionless projects alphabetically. Ties are broken by name for fully deterministic ordering.

- [`a631b4f`](https://github.com/tsiao/devo/commit/a631b4faa2a662fc102c22a911969fb22b8e151f) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix session title stretching to full width in the app bar. The title element now stays inline with the breadcrumb instead of expanding to fill all available space.

- [`26f8c07`](https://github.com/tsiao/devo/commit/26f8c0719468a80cf38365086d33ccefdbbd4d4b) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix excessive API requests on the worktree settings page. The worktree list was re-fetched for every connected project on each session update because the effect depended on volatile fields (`agentCount`, `lastActiveAt`). Now the fetch is gated on a stable directory key that only changes when projects are added or removed.

## 0.7.1

### Patch Changes

- [`7b1f502`](https://github.com/tsiao/devo/commit/7b1f5024766f2de4844cadae8b1b556b1bacfa13) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Add duration timing to IPC fetch proxy and handler logging, with slow handler warnings above 500ms

- [`7b1f502`](https://github.com/tsiao/devo/commit/7b1f5024766f2de4844cadae8b1b556b1bacfa13) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Extend mock mode with diff data support for review panel testing

- [`7b1f502`](https://github.com/tsiao/devo/commit/7b1f5024766f2de4844cadae8b1b556b1bacfa13) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix review panel: reset to closed on app start instead of persisting, auto-close when navigating to sessions with no diffs, and fix diff loading state tracking

## 0.7.0

### Minor Changes

- [`fd5e3c1`](https://github.com/tsiao/devo/commit/fd5e3c176e654a34691f995c3a8d6785873aaee9) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Add review panel for viewing session file diffs with syntax highlighting offloaded to web workers, virtualized diff list, pinned file headers, collapsed unchanged context lines, slide-in animation, inline code review comments that get prepended to chat messages, and a "View diff" button on edit tool cards to jump directly to a file in the panel

### Patch Changes

- [`fd5e3c1`](https://github.com/tsiao/devo/commit/fd5e3c176e654a34691f995c3a8d6785873aaee9) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Improve automation executor reliability with SDK call timeouts, structured logging throughout the execution pipeline, stale timer guard in the scheduler, and fallback rrule computation when the DB next-run value is missing on startup

## 0.6.0

### Minor Changes

- [`f7c84b5`](https://github.com/tsiao/devo/commit/f7c84b5b4162824de5b4d31e860a6658f875e65e) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Add dedicated Linux tray icon, lazy session loading with pagination and project search for improved sidebar performance, and client-side first-seen timestamps for accurate tool call durations

### Patch Changes

- [`f7c84b5`](https://github.com/tsiao/devo/commit/f7c84b5b4162824de5b4d31e860a6658f875e65e) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix automation scheduler to use async next-run-time computation and prefer in-memory values over stale DB entries, cap task list height with auto-scroll on updates, and update sidebar search button to command palette icon

- [`f7c84b5`](https://github.com/tsiao/devo/commit/f7c84b5b4162824de5b4d31e860a6658f875e65e) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Extract ChatInputSection into its own component, optimize sub-agent card rendering, move detailed session metrics (tokens, exchanges, tools) into a popover for a cleaner toolbar, and add IPC fetch request/response logging

- Updated dependencies [[`f7c84b5`](https://github.com/tsiao/devo/commit/f7c84b5b4162824de5b4d31e860a6658f875e65e)]:
  - @devo/ui@0.6.0

## 0.5.3

### Patch Changes

- [`08bce3e`](https://github.com/tsiao/devo/commit/08bce3ebce7a7a0721770c246e38b783154f44ac) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix missing app icon in Linux dock and app selector (GNOME/Fedora) by adding StartupWMClass to the desktop entry, setting an explicit executable name, fixing the BrowserWindow icon path for packaged builds, and providing a multi-size icon set

- [`8171dee`](https://github.com/tsiao/devo/commit/8171deee90b638ea92604a20ab323da3f739b627) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix horizontal overflow and clipping in the desktop app at narrow window widths. Add `min-w-0` and `overflow-hidden` throughout the flex layout chain (SidebarInset, content area, conversation container, chat view, prompt toolbar) and make the session app bar collapse responsively with Tailwind breakpoints.

- Updated dependencies [[`8171dee`](https://github.com/tsiao/devo/commit/8171deee90b638ea92604a20ab323da3f739b627)]:
  - @devo/ui@0.5.3

## 0.5.2

### Patch Changes

- [`57ddb2f`](https://github.com/tsiao/devo/commit/57ddb2fdb750f9306c2710fa898bb7d3509c9796) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix splash screen persisting on top of onboarding on first launch

- [`57ddb2f`](https://github.com/tsiao/devo/commit/57ddb2fdb750f9306c2710fa898bb7d3509c9796) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Use semver library for Devo version compatibility checks with standard range syntax (supported: `>=1.2.0`, tested: `~1.2.0`)

- Updated dependencies [[`098847c`](https://github.com/tsiao/devo/commit/098847c404f51c0954ffaba1c872910b93dd69d9)]:
  - @devo/configconv@0.5.2

## 0.5.1

### Patch Changes

- [`aa529d4`](https://github.com/tsiao/devo/commit/aa529d4b9e2887ecbad2e34b6a7d372ae4f085c8) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix macOS auto-updater failing with code signature validation error on unsigned builds

## 0.5.0

### Minor Changes

- [`67818d0`](https://github.com/tsiao/devo/commit/67818d0ba51ba07b32bf850a01179f94858dabc6) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Short, user-facing description of the change. One paragraph is ideal. Use
  markdown formatting sparingly (bold for emphasis, backticks for code).

  ````

  ### Frontmatter rules

  - Keys are **quoted package names** exactly as they appear in `package.json`.
  - Values are `patch`, `minor`, or `major` (standard semver).
  - A single changeset can list multiple packages if they are part of the same
    logical change:
    ```yaml
    ---
    "@devo/desktop": minor
    "@devo/configconv": minor
    ---
  ````

  ### Body rules

  - Write for end users, not developers. Focus on what changed, not how.
  - One concise paragraph (1-3 sentences). No headings, no bullet lists.
  - Avoid repeating the package name; the changelog groups entries by package.
  - Use backticks for paths, flags, and code references.
  - Do NOT use em dashes.

  ## One changeset per logical change

  **Each distinct feature, fix, or improvement gets its own changeset file.** Do
  NOT combine unrelated changes into a single changeset. This keeps changelog
  entries scannable and makes it easy to see what shipped in a release.

  Good (3 separate files):

  ```
  .changeset/
    worktree-api-migration.md    -> "Migrate worktree management to Devo API"
    automation-execution.md      -> "Add automation execution engine with SDK"
    streaming-fix.md             -> "Fix non-streaming parts not triggering re-renders"
  ```

  Bad (1 giant file):

  ```
  .changeset/
    big-release.md               -> "### New Features\n- Worktrees\n- Automations\n### Fixes\n- ..."
  ```

  ## File naming

  Use `kebab-case` names that describe the change. The name does not affect
  behavior but helps humans identify what each changeset covers. Examples:

  - `worktree-api-migration.md`
  - `fix-streaming-rerenders.md`
  - `add-context-usage-indicator.md`

  ## Bump type guidelines

  | Type    | When to use                                                             |
  | ------- | ----------------------------------------------------------------------- |
  | `patch` | Bug fixes, refactors with no user-visible behavior change               |
  | `minor` | New features, new UI, new configuration options                         |
  | `major` | Breaking changes (removed features, changed defaults, new requirements) |

  When in doubt, prefer `minor` for anything user-visible and `patch` for
  internal improvements.

  ## Linked packages

  This project links all five workspace packages together (see `config.json`):

  ```
  @devo/desktop, @devo/ui, @devo/server, @devo/configconv, configconv
  ```

  "Linked" means: when multiple linked packages are bumped in the same release,
  they all receive the **same final version** (the highest bump wins). Packages
  not mentioned in any changeset are left at their current version.

  ## Workflow

  ### Adding changesets (during development)

  1. After completing a logical change, create a new `.md` file in `.changeset/`.
  2. List the affected packages and bump types in the frontmatter.
  3. Write the changelog summary in the body.
  4. Commit the changeset file alongside your code changes (or in a follow-up
     commit on the same branch).

  ### Automated release pipeline (GitHub Actions)

  Version bumping, changelog generation, and releasing are **fully automated** by
  two GitHub workflows. Agents and developers should never run `changeset version`
  manually.

  **On push to `main` with pending changesets** (`.github/workflows/release.yml`):

  1. The `changesets/action` detects pending `.md` files in `.changeset/`.
  2. It runs `bun changeset version` automatically, which consumes the changeset
     files, bumps `package.json` versions, and updates `CHANGELOG.md` files.
  3. It opens (or updates) a **"chore: version packages"** PR with all the
     version bump changes.
  4. A maintainer reviews and merges that PR.

  **On merge of the version PR** (same workflow, second run):

  1. The `changesets/action` detects no pending changesets and runs
     `bun changeset tag`, which creates git tags for the new versions.
  2. The workflow reads the new version from `apps/desktop/package.json`.
  3. It triggers cross-platform Electron builds (Linux, macOS, Windows).
  4. It creates a GitHub Release with the changelog body and all build artifacts.

  **On PRs targeting `main`** (`.github/workflows/changeset-check.yml`):

  - A check warns if no changeset file is present, reminding contributors to add
    one for user-facing changes.

  ### Manual release (escape hatch)

  Use `workflow_dispatch` on the Release workflow to force a build and release
  from the current `package.json` version. This is for recovery or re-releases
  only, not normal operation.

  ### What NOT to do

  - Do NOT run `changeset version` or `changeset tag` locally. The GitHub Action
    handles this, and running it locally causes conflicts with the automated PR.
  - Do NOT manually edit `CHANGELOG.md`; let the tool generate it.
  - Do NOT use `changeset add` interactively in agent sessions (it requires
    `/dev/tty`). Create the `.md` files directly instead.
  - Do NOT create empty changesets (no packages listed). Use `--empty` only for
    documentation-only changes that need no version bump.
  - Do NOT push version bump commits manually. Let the "Version Packages" PR
    handle the version/changelog/tag lifecycle.

  ## Config reference

  The `config.json` in this folder controls behavior:

  | Key                          | Value                          | Meaning                                      |
  | ---------------------------- | ------------------------------ | -------------------------------------------- |
  | `changelog`                  | `@changesets/changelog-github` | Generates GitHub-linked changelog entries    |
  | `commit`                     | `false`                        | `changeset version` does not auto-commit     |
  | `linked`                     | all 5 packages                 | Bumped packages share the same version       |
  | `baseBranch`                 | `main`                         | Changesets are compared against `main`       |
  | `updateInternalDependencies` | `patch`                        | Internal dep ranges are bumped automatically |
  | `privatePackages`            | version + tag                  | Private packages still get versions and tags |

- [`9fd5fc1`](https://github.com/tsiao/devo/commit/9fd5fc184e2ef51813d85ececef4e81e140aa4d3) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Add a real automation execution engine powered by the Devo SDK. Automations now create Devo sessions with configurable permission presets, model resolution, retry logic with exponential backoff, and live session tracking. Execution results are persisted to the SQLite database and automation storage follows XDG Base Directory conventions (`~/.config/devo/automations/`).

- [`9fd5fc1`](https://github.com/tsiao/devo/commit/9fd5fc184e2ef51813d85ececef4e81e140aa4d3) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Redesign the automations UI: new schedule picker with visual cron builder, project combobox in the creation dialog, nested routes with a dedicated detail view, toast notifications for automation actions, sidebar link with feature flag toggle, and server-awareness to hide the menu item on remote servers.

- [`9fd5fc1`](https://github.com/tsiao/devo/commit/9fd5fc184e2ef51813d85ececef4e81e140aa4d3) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Add context window usage indicator with circular progress display in the status bar, compaction threshold tooltip, and improved session metrics that track turns as request-response exchanges with accurate message counts and timer precision.

- [`9fd5fc1`](https://github.com/tsiao/devo/commit/9fd5fc184e2ef51813d85ececef4e81e140aa4d3) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Add configurable local server hostname, port, and password in settings. Harden server switching and discovery with health checks, health-probe non-active servers when the popover opens, and show empty states when the server is offline or no projects exist.

- [`abf40c5`](https://github.com/tsiao/devo/commit/abf40c588b59d79488c00b28807e7dd73f4a706e) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Add a two-layer startup splash screen with phase-based status messages ("Starting server...", "Connecting...", "Loading projects..."). A transparent HTML splash renders instantly before JS loads, then hands off to a React overlay that fades out once discovery completes. Both layers are transparent so macOS liquid glass and vibrancy effects show through.

- [`9fd5fc1`](https://github.com/tsiao/devo/commit/9fd5fc184e2ef51813d85ececef4e81e140aa4d3) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Migrate worktree management from a custom Electron-based implementation to Devo's native worktree API. The legacy `worktree-manager.ts` in the main process has been removed and replaced with a renderer-side `worktree-service.ts` that calls the Devo SDK directly. Sandbox projects created from worktrees now merge into the parent project in the sidebar.

### Patch Changes

- [`abf40c5`](https://github.com/tsiao/devo/commit/abf40c588b59d79488c00b28807e7dd73f4a706e) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Resolve the shell environment asynchronously at startup. The window now opens immediately while the login shell spawns in the background, removing a blocking delay on macOS and Linux.

- [`9fd5fc1`](https://github.com/tsiao/devo/commit/9fd5fc184e2ef51813d85ececef4e81e140aa4d3) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix ESM-compatible `__dirname` in CLI install and tray modules. Replace `viteStaticCopy` with a custom Rollup plugin for copying Drizzle migration files. Replace local type definitions with imports from the Devo SDK.

- [`9fd5fc1`](https://github.com/tsiao/devo/commit/9fd5fc184e2ef51813d85ececef4e81e140aa4d3) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix non-streaming part updates not triggering re-renders by bumping the internal version counter. Add subtle background to tool cards for better visual separation. Remove user prompt from agent card expanded view.

- [`9fd5fc1`](https://github.com/tsiao/devo/commit/9fd5fc184e2ef51813d85ececef4e81e140aa4d3) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Improve worktree UX: non-blocking launch with a stub session and background setup, space-themed random name generator (replacing manual slug input), improved commit dialog layout, and "apply to project" now targets the project directory correctly.

- Updated dependencies [[`755242d`](https://github.com/tsiao/devo/commit/755242d87f361457d00f3d56b91002f5ee1a7a6e)]:
  - @devo/configconv@0.5.0

## 0.4.1

### Patch Changes

- [`32dad30`](https://github.com/tsiao/devo/commit/32dad30e9f65e1991f344b606f591753d3739099) Thanks [@ItsWendell](https://github.com/ItsWendell)! - Fix "Add Project" for remote servers: show a path text input dialog instead of the native folder picker (which only shows the local filesystem). Also makes the automation dialog's project picker remote-aware with an inline path input.

## 0.4.0

### Minor Changes

- [`2a9d3de`](https://github.com/tsiao/devo/commit/2a9d3de3b529a6aa73b8ae574fb2a6f2084d73f9) Thanks [@ItsWendell](https://github.com/ItsWendell)! - ### Breaking: Require Devo >= 1.2.0

  Devo now requires Devo 1.2.0 or higher. Older versions will be blocked during the environment check.

  ### New Features

  - **Incremental streaming (message.part.delta)**: handle the new `message.part.delta` SSE event for incremental text/reasoning updates, replacing full part object replacements with efficient string delta appending
  - **Remote server support**: connect to remote Devo servers with authentication, mDNS discovery, and onboarding integration
  - **Update banner redesign**: floating toast card instead of full-width bar

  ### SDK Migration

  - Upgrade `@devo-ai/sdk` from 1.1.x to 1.2.x across all packages
  - Migrate all SDK type imports from v1 to v2, gaining proper typed discriminated unions for all events
  - `Permission` type replaced by `PermissionRequest` (field `title` becomes `permission`)
  - `permission.updated` event replaced by `permission.asked`
  - Remove unnecessary type casts throughout the event processing pipeline

  ### Fixes

  - Fix CI release workflow to run builds in a single workflow run
  - Suppress hover background on macOS sidebar server indicator

### Patch Changes

- Updated dependencies [[`2a9d3de`](https://github.com/tsiao/devo/commit/2a9d3de3b529a6aa73b8ae574fb2a6f2084d73f9)]:
  - @devo/configconv@0.4.0

## 0.3.0

### Minor Changes

- [`594e4b7`](https://github.com/tsiao/devo/commit/594e4b7e299dee6ba507f990001f505f6afd22c5) Thanks [@ItsWendell](https://github.com/ItsWendell)! - ### New Features

  - **Session metrics**: comprehensive per-session work time, cost, tokens, model distribution, cache efficiency, and tool breakdown
  - **Per-turn metadata**: display model, duration, and cost after each response
  - **Live turn timer**: show elapsed time on submit button while agent is working
  - **Prompt toolbar selector**: enhanced model/provider selector UI
  - **Devo API integration**: replace disk-based discovery and messages with Devo server API

  ### Performance

  - **Per-session streaming**: scope streaming buffer and version notifications per session, eliminating cross-session re-renders (~20x/sec savings for inactive sessions)
  - **Decoupled volatile metrics from agent identity**: sidebar no longer re-renders on every metrics tick
  - **Lazy metrics for background sessions**: only the viewed session uses reactive atom subscriptions
  - **Granular sidebar subscriptions**: each project folder and session item subscribes independently

  ### UI/UX

  - **Base UI migration**: migrated shadcn components from Radix to Base UI
  - **Sidebar improvements**: stabilized sort order, relative last-active time, animated project folders
  - **New chat styling**: translucent backgrounds and subtle borders on suggestion cards
  - **Command dialog**: layout fix for better usability

  ### Fixes

  - Support Devo XML read output format in tool card parser
  - Migrate tooltips from Radix asChild to Base UI render prop pattern
  - Stabilize sidebar atom references to prevent cascading re-renders

### Patch Changes

- Updated dependencies [[`594e4b7`](https://github.com/tsiao/devo/commit/594e4b7e299dee6ba507f990001f505f6afd22c5)]:
  - @devo/ui@0.3.0

## 0.2.0

### Minor Changes

- [`d2d6f2b`](https://github.com/tsiao/devo/commit/d2d6f2b3013ad0fa3bb9ac08ad9b8ff91517ffc5) Thanks [@ItsWendell](https://github.com/ItsWendell)! - ### New Features

  - Add provider management with icons, catalog, and onboarding integration
  - Add git worktree backend with lifecycle management and UI
  - Add automations subsystem with database, scheduler, and IPC (hidden for now)

  ### Improvements

  - Improve Electron main process reliability
  - Improve command palette animation and chat UX
  - Increase spacing between expanded tool call items in chat turns

  ### Fixes

  - Upgrade hono to 4.11.9 to resolve security alerts
  - Resolve type errors in chat-input component

### Patch Changes

- Updated dependencies [[`d2d6f2b`](https://github.com/tsiao/devo/commit/d2d6f2b3013ad0fa3bb9ac08ad9b8ff91517ffc5)]:
  - @devo/ui@0.2.0
