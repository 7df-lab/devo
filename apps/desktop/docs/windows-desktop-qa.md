# Windows desktop QA

## Model picker and remote catalog QA

Tested through the actual Electron UI on 2026-10-09:

- `/model` opened the same picker in new chats and existing conversations.
- Ready to use and All models showed connected and unconnected providers.
  Provider/model search narrowed ChatGPT results; Enter selected the model.
  The refreshed catalog showed 1,279 entries with 60 rows rendered initially.
- Selecting an unconnected ChatGPT model opened the existing browser OAuth
  flow. Authentication completed in the user's browser; Computer Use did not
  automate authentication. The desktop subsequently showed 14 ChatGPT models
  as ready. Fixed an OAuth state reset caused by catalog object refetches.
- Independently submitted a message using GPT-5.6 Luna and observed a completed
  assistant reply. A separate Codex turn executed real Python with a 25-second
  asyncio wait. Queued a message during execution and clicked Steer. Native
  persistence recorded it as a `steer` user item in the same active turn, and
  the completed answer acknowledged it. Another queued follow-up completed.
  Renderer reload preserved both follow-ups, replies, and the selected model.
- Clicked Refresh in Electron. The Rust server downloaded
  `https://models.dev/api.json`; cache metadata recorded 24 overlapping providers
  and 770 converted remote models. The picker remained usable and returned from
  Updating to Refresh. Built-in entries and user overrides remain merged.
- The ChatGPT account rejected GPT-5.4 with an explicit unsupported-model error;
  GPT-5.6 Luna was used for successful conversation and tool QA. A global catalog
  is not a guarantee of account-specific model access.

Validation for this addition: 865 desktop tests passed with one platform skip;
TypeScript passed in both checkouts. Native catalog integration tests passed
for HTTP fetching, TTL reuse, forced refresh, offline policy, and fallback
after failure. Seven protocol generation tests passed. Windows workspace
Clippy and Cargo build passed with `RUSTFLAGS=-Dwarnings`, including the user's
main checkout build.

Tested on Windows on 2026-10-09 using an isolated checkout, backend home,
Electron profile, and temporary Git project. The existing user checkout and
open desktop session were preserved. This covers the desktop renderer,
Electron main/preload, Native protocol, Rust runtime, and Python kernel.

## Actual Electron interaction

These checks used Windows computer use against the running Electron window.
They were supplemented by protocol traces and filesystem checks, rather than
replaced by scripted browser end-to-end tests.

| Scenario | Observed outcome |
| --- | --- |
| Start `bun run dev` | Native runtime connected; main, preload, and renderer started. |
| Rebuild with running Windows runtimes | Two original checkout CLI processes remained running. The new `bun run dev` launched from a unique runtime copy, and a forced `bun run dev:cli` rebuild succeeded with the app still open. The isolated runtime also rebuilt with `RUSTFLAGS=-Dwarnings` while Electron was connected. |
| Add a temporary Git project | Native folder chooser accepted the project; sidebar and composer opened. |
| Select a connected model | Found and fixed missing inherited provider models in Native preferences. |
| Send a greeting with Markdown | Assistant text, table, and Python code rendered; one user bubble. |
| Run a 90-second Python cell | Running activity, Stop control, and Python source/output were visible. |
| Send another message during work | Queue entry appeared with Edit/Remove/Steer controls. |
| Steer the queued message | Native trace recorded `entry: steer` in the active turn; assistant subsequently answered the follow-up. |
| Earlier automatic queue and steer checks | Queued follow-up drained automatically; a separate steer was acknowledged during active work. |
| Edit a fixture file | Agent wrote the requested 26-byte text; Changes displayed the Git diff. |
| Navigate with an unsent draft | Draft survived leaving and returning to the conversation. |
| Reload and restart with a draft | Found reload loss; fixed eager storage initialization and verified persistence. |
| Restart the runtime | Fixed stale actor/subscription caches and stopped event forwarding; a fresh Electron retest resumed the session, displayed one provider error, and returned to idle. |
| Distinguish duplicate model names | Found indistinguishable configured connections; duplicate names now show a readable model/connection identifier. |
| Restore Python state after editing | Found a closed write handle reopening and truncating the fixture; blocked file reducers and verified the repaired fixture remained intact after restore. |
| Provider failure | HTTP 402 was visible as “Insufficient provider balance,” with expandable details. |
| Funded recovery | A new real Electron request produced assistant output, Python output, and a fixture read after balance was replenished. No new 402 occurred. |
| Funded automatic queue | Enter queued a follow-up during a 60-second Python cell; it drained automatically and produced its own Python result and assistant reply. |
| Mouse submission during work | Clicked Queue message during a fenced 150-second Python cell. Edited and resubmitted the first queued entry, then queued and steered a second. The steer produced 402; the edited queue drained automatically and produced 301. Stop remains separately available. |
| Completed activity label | Removed the tool suffix: completed activity displays “Worked for 1s,” with Python details inside the disclosure. |
| Separate inner cells | Clicked Worked to reveal collapsed Thought and Python rows. Opened each independently and collapsed the whole group with Worked. Python source and output stay paired. |
| Steering visibility after completion | A fresh steer during a 90-second Python wait remained a user bubble after completion, runtime restart, and renderer reload. The assistant applied it and produced 503. The reported disappearance was not reproduced in this checkout; live/replay regression tests preserve it. |
| Funded Stop and recovery | Clicked Stop during a 120-second cell; activity displayed Stopped and the composer returned to idle. A fresh Python request read the intact 26-byte fixture and produced 603. |
| Python after runtime restart | Found that asyncio was omitted from snapshots but not recreated by bootstrap. Added its bootstrap import and regression coverage for fresh and restored kernels. |
| Light/dark and narrow chat column | Reviewed wrapping, composer controls, warning text, activity disclosures, and diff layout. |
| Search, copy, stop/recovery | Exercised in the earlier live Electron pass associated with this PR. |

The initial isolated QA home had no provisioned OS sandbox, so its warning
was accurate. A subsequent setup request was canceled at Windows administrator
consent. The QA home now reuses the user's valid encrypted provisioning
artifacts and their access rules, preserving destination ownership. A real
product-binary probe ran as `DevoSandboxOffline`: a workspace write succeeded,
an outside write was denied, and outbound TCP was denied with Windows error
10013. The outside sentinel remained unchanged. After restarting the runtime,
a fresh real Electron Python turn reported the token `DevoSandboxOffline`,
used the fenced kernel, and emitted no new unfenced warning. Historical
warnings remain attached to their original turns.

Funded Electron requests now succeed. The earlier 402 checks remain coverage
of provider failure handling, rather than a current provider-balance blocker.

## Fixes and regression coverage

- Replace Windows asyncio's socket wakeup with an IOCP named pipe, avoiding
  both blocked loopback TCP and writable-temp-directory probes.
- Accept startup stderr until the kernel ready event; retain complete startup
  diagnostics on failure.
- Skip disk-backed file handles in Python snapshots, including nested handles;
  reject legacy file reducers before they reopen or truncate files. Preserve
  other bindings and in-memory buffers.
- Retain one session rollout lock per process until its last owner drops,
  while preserving cross-process exclusion on Windows.
- Include inherited models for configured providers in Native preferences.
- Reinitialize loaded sessions after transport closure; retry a missing actor
  with the same prompt and idempotency key, and recreate its subscription.
- Keep Electron Native event listeners attached across backend replacement;
  reconnect project event streams after closure.
- Persist drafts before initial composer reads; retain input when submission
  fails.
- Allow mouse submission of follow-ups during work with an explicit Queue
  message button; keep Stop independent of submission readiness.
- Simplify completed activity summaries to elapsed time without a tool suffix.
- Keep separate, independently expandable Thought/tool cells inside the
  collapsed Worked summary; test actual disclosure interaction.
- Recreate asyncio during kernel bootstrap so snapshot restoration retains
  the documented asynchronous Python namespace.
- Display actual warning text and keep replayed provider errors scoped to
  their original turn.
- Persist actual turn counts instead of Unix timestamps.
- Normalize generated-file comparisons across line endings, use native paths
  in Windows fixtures, create required test workspaces, and exercise the CLI
  sandbox entry point instead of relaunching a Rust test harness.
- Map generic ACL masks consistently, fix Windows-only Clippy warnings, and
  use bounded observable waits in runtime concurrency tests.
- Terminate the command runner's child when parent IPC closes or sends an
  invalid frame; test both disconnect paths using a real child process.
- Resolve Windows short-path fixture aliases and route protocol validation
  imports to the freshly generated SDK source on clean installs.

## Repeatable validation

### Removing a folder with working sessions

Windows computer use created three chats in a new temporary project. Each
ran a real Python cell with `await asyncio.sleep(600)`. A fourth message was
queued while the third cell was running. Clicking the project's Remove
confirmation returned to the home screen in 817 ms, including computer use
observation overhead.

All three Python processes and their sandbox command runners exited. The
three session index rows, pending messages, and rollout files were gone;
the project's `keep.txt` file and three chats in another project remained.
Reloading Electron did not restore the deleted folder or its sessions.

Regression tests cover the eight-request deletion pool, partial failures,
empty folders, twelve simultaneous Native turns with queued and steering
inputs, deletion broadcasts, forced abortion of twelve noncooperative tasks,
completion waiter cleanup, and termination of a live Python cell. Linux coverage also checks deletion
of an idle session whose background command belongs to another connection.
The confirmation now explains interruption, and the Windows project menu
uses “Show in Explorer.”

The latest desktop suite passed 852 tests with one platform skip. TypeScript
and the production main/preload/renderer build passed. The full Rust workspace
run passed 2,413 tests with 56 ignored and zero failures. Workspace build and
Clippy passed with warnings treated as errors.

### Activity hierarchy follow-up

Windows computer use verified a completed turn's Worked summary opens separate
Thought and Python rows. Thought expanded without Python code, Python expanded
with Thought closed, and closing Worked removed both inner controls. A new live
35-second Python turn included assistant commentary between reasoning and tool
execution. Its activity stayed in one group. Leaving Python expanded while
running and letting the turn finish closed the group under Worked for 36s;
commentary and the final answer stayed visible. The older combined panel in the
user checkout was updated with the same independent rows and turn grouping.

Before the folder deletion fix, the local serial Rust workspace run passed 2,410 tests with 56 ignored
and no failures. Desktop tests passed 847 tests with one platform skip;
TypeScript passed. All 11 Python tests passed. Workspace build and Clippy
completed with warnings treated as errors; rustfmt was clean.

From the repository root, with `RUSTFLAGS=-Dwarnings`:

```text
cargo build --workspace --jobs 2
cargo test --workspace --jobs 2 --no-fail-fast -- --test-threads=1
cargo clippy --workspace --all-targets --jobs 2 -- -Dwarnings
cargo fmt --all -- --check
```

From `apps/desktop`:

```text
bun run test
bunx tsc --noEmit
bun run build
bun run dev
```

Python, with `dill==0.4.1` installed and
`PYTHONPATH=crates/kernel/rlm-runtime/src`:

```text
python -m unittest discover -s crates/kernel/rlm-runtime/tests -v
```

CI includes desktop tests/types, Windows Rust check/Clippy, Python 3.11/3.13
kernel tests, and the existing Linux Rust build/test/format/lint/docs jobs.
The PR is kept as a single amended commit.

Windows desktop development uses a separate Cargo output and per-launch
runtime copies to avoid rebuilding a locked executable. See
[Desktop development](development.md) for paths, overrides, and cleanup.

## Settings QA and connection retention (2026-10-09)

QA used the actual Electron UI through Windows computer use, with a separate
Electron profile, DEVO_HOME, Git workspace, local provider, MCP echo server, and
user skill. User checkout changes were synchronized without discarding unrelated
work. Existing user Electron processes and credentials were preserved.

- Full Electron close/reopen retained ChatGPT, DeepSeek, and a custom provider in
  Ready to use. GPT-5.6 Luna completed a new real request after reopening.
- General: tested themes, display mode, thinking preference, opaque window
  recreation, and open destinations. Fixed internal IDs appearing as labels and
  development relaunch quitting Vite before its replacement window could load.
- Notifications: tested completion mode and permission/question/error toggles;
  preferences persisted in the QA profile across process restarts.
- About: development update checks now explain their unavailability and disable
  the action, instead of silently doing nothing.
- Servers: runtime restart succeeded. Invalid custom proxy input now disables
  Restart; valid SOCKS input enables it. System proxy was restored for live QA.
- Providers/Models: created a local connection, discovered a second remote model,
  verified the open dialog refreshed, toggled a model, and saved its display name.
  Provider details include inherited models; failed actions show retryable errors.
  Actual ChatGPT remote discovery exposed and fixed OAuth bearer/account handling,
  sparse endpoint/protocol inheritance, provider naming, and hidden-model handling.
- MCP: the QA echo server advertised a tool, disabled state survived restart,
  and reenable returned it to ready. Skills: the QA user skill's disabled state
  survived restart and could be reenabled.
- Worktrees: replaced SDK placeholders with Native Git operations; list uses
  actual linked checkouts and Windows names. Reset/remove require confirmation,
  refuse dirty/primary/working checkouts, and show failures. Real Git lifecycle
  tests cover create/list/reset/remove. UI testing found inherited stdin blocking
  Git for Windows; commands now explicitly isolate stdin from Native transport.
- Setup: runtime check detected bundled 0.1.39, and Re-run Setup completed all
  onboarding steps and returned to the application without losing connections.
  Migration restore was not applied to the user's configuration.

Final local Desktop validation: 878 tests passed, one platform skip, zero failures;
TypeScript and the production build passed. Rust persistence, discovery (nine),
real Git lifecycle, and Python goal-completion regressions passed. Windows builds
and Clippy ran with warnings denied. The CI goal-completion fixture now allows
30 seconds for real Python startup and reports unexpected provider requests.


## Unified model controls and effort save follow-up (2026-10-09)

- Consolidated Settings Models/Providers into Models & providers, embedding the
  composer picker. Connections and advanced model configuration remain in this
  destination; old model links still work.
- Actual Electron UI selected GPT-6 Luna and Low effort in Settings, showed Saved
  for new chats, then showed GPT-6 Luna / Low in the new-chat composer.
- Cold-start QA reproduced model/preferences/read exceeding the old 10-second
  transport budget and delaying other RPCs. Effort projection now reads the loaded
  catalog instead of resolving full turn configuration repeatedly.
- Default selection writes use the canonical preference RPC directly, serialize
  model/effort saves, preserve qualified model IDs, and allow retry after errors.
  Local project model history cannot override Native defaults on a new chat.
- 886 Desktop tests passed, one platform skip, zero failures; TypeScript passed.
  The Native stdio regression saved effort before creating a chat and verified
  the complete response and an empty session list. Windows Cargo build passed
  with warnings denied.


## Session loading and console QA (2026-10-09)

- Used an isolated Electron profile, Native home, and 120-session folder with a
  2,400-item transcript. Actual computer-use QA covered pagination, folder
  collapse/reopen, rapid navigation, Settings, real Codex Python execution,
  queued input, live steering, and conversation restoration after restart.
- Initial transcript RPCs fell from twelve 200-item pages to one 160-item tail
  page. Older history loads in bounded increments. Idle sessions use a folder
  roster instead of individual replay subscriptions; queue readiness does not
  load transcripts. Individual warm-reload observations were 5.4s before and
  2.5s after, without claiming a general benchmark.
- Fixed a queued-resume self-deadlock, recovery reads under the connection
  barrier, startup folder loading before connection readiness, stale navigation
  completion state, deprecated Jotai imports, shared state updates during layout
  rendering, and Worktrees errors for ordinary folders.
- The final Electron capture contained zero renderer errors and zero Native
  request failures; the Vite development CSP warning remains expected. Queued
  and steered instructions remained visible after completion and restart, and
  the real model followed both instructions.
- Local validation: 896 Desktop tests passed, one platform skip, zero failures;
  TypeScript, production build, Rust formatting, and workspace Clippy passed.
  Rust server library: 475 passed, two ignored; Native concurrency integration:
  two passed; protocol generation: seven passed. The user checkout's plain
  Cargo build passed with `RUSTFLAGS=-Dwarnings`.
- A pre-fix deadlock reproducer remains running in the isolated QA worktree:
  repository instructions prohibit interrupting Rust commands. Later regression
  tests have request timeouts and pass; the old reproducer does not lock the
  user checkout's executable.

The Native cursor and subscription contract is implemented in the server session handlers.
