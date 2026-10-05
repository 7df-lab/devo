# Devo Manual Test Log (Phase 1)

Date: 2026-09-16
Binary: `target/debug/devo.exe` (v0.1.39)

## Test matrix

| ID | Area | Scenario | Status | Notes |
|----|------|----------|--------|-------|
| T01 | Startup | Fresh session boots, prompt visible | pending | |
| T02 | Chat | Short reply turn | pending | |
| T03 | Slash | `/model` open/Esc | pending | |
| T04 | Slash | `/effort` open/Esc | pending | |
| T05 | Slash | `/system-prompt` | pending | |
| T06 | Slash | `/goal` status + set + clear | pending | |
| T07 | Slash | `/refine` if present | pending | |
| T08 | Slash | `/compact` if present | pending | |
| T09 | Slash | `/skill:` listing | pending | |
| T10 | Theme | theme picker dark/light only | verified | `/theme` opens dark/light selector; `/theme dark` applies; autocomplete shows `/theme [dark|light]` |
| T31 | Logs/import/export | rollout JSONL parity | verified | `/logs` lists diagnostic + `~/.devo/sessions` rollouts; `/session` shows File path; `/export` writes jsonl; `/import` rewrites sessionIds, skips workspace artifacts, resume loads items (stdio e2e) |
| T11 | RLM | ipython arithmetic | pending | |
| T12 | Memory | create_memory + overview path | verified | 2026-09-17 isolated DEVO_HOME: `create_memory(QA_MEM_DIRECT)` → overview + `session-artifacts/…/harness/harness_state.json` |
| T13 | Refine | refine.status/run/pending | verified | `refine.run(...QA_MEM_REFINE...)` scheduled:True; post-turn apply wrote `qa_mem_refine` + refinements[] (`source: refine`) |
| T14 | Goal skill | goal.create/get/complete | pending | |
| T15 | Compact | compact.status/run | pending | |
| T16 | Files | write file via python host | pending | |
| T17 | Files | per-tool diffs vs workspace changes | pending | |
| T18 | Shell | long-running bash | pending | |
| T19 | Interrupt | Esc mid-turn | pending | |
| T20 | Queue | Alt+Enter / Escape Enter follow-up | pending | |
| T21 | Resume | exit + Left Agents View resume | pending | |
| T22 | Multi-turn | tool then resume continuity | pending | |
| T23 | Subagent | spawn child + parent message | pending | |
| T24 | Image | attach-image / vision path | pending | |
| T25 | Mermaid | render mermaid block | pending | |
| T26 | Error | invalid refine on child | pending | |
| T27 | Plan mode | mutation denied | pending | |
| T28 | Global refine | global_=True memory | verified | `create_memory(..., global_=True)` → `DEVO_HOME/harness/`; new session local empty; `overview(global_=True)` has `QA_MEM_GLOBAL` |
| T29 | Rollback | refine rollback_id if exposed | pending | |
| T30 | Heartbeat | rlm_heartbeat + `/heartbeat` | verified | 2026-09-17: RLM create/list/pause/resume/delete + wake (`runCount`); `/heartbeat` via server `session/heartbeat/command` (parse/apply backend-owned) |

## Defects

### D01 — `/goal` bare status easy to miss
- Repro: send `/goal` with no args after other chat.
- Observed: status can be easy to miss amid prior content; first bare `/goal` after `/system-prompt` showed no clear status in capture.
- Severity: Low UX

### D02 — `await goal.complete()` does not stop pursuit / turn thrash **[FIXED]**
- Root cause: `/goal` starts a GoalContinuation turn; `host_goal_complete` called `interrupt_active_goal_continuation_turn`, which cancelled the same turn mid-host_request (cell ✗) and raced UI updates.
- Fix: `clear_goal_continuation_registration` (no self-interrupt) + `GoalUpdated`/`GoalStatusChanged` broadcasts.
- Verified: `goal.complete()` ✓, “Goal complete” announcement, tray no longer “Pursuing goal”.

### D03 — Workspace “files changed” counter jumps wildly **[FIXED]**
- Root cause: footer seeded with full `uncommitted` worktree (1000+ files in this repo).
- Fix: turn-complete uses turn-scoped recap; resume clears recap instead of scanning uncommitted.

### D04 — Escape alone unreliable mid-tool; need C-c + Esc **[FIXED]**
- Root cause: abort() awaited session/interrupt before agent_end; cancel dropped ipython without kernel.interrupt().
- Fix: optimistic abort UI; router + handler call kernel.interrupt() on cancel.

### D05 — Queue follow-up: FIRST prompt may get no assistant reply **[HARNESS]**
- Prior repro used Escape+Enter as Alt+Enter; Escape interrupts the in-flight turn. AGENTS.md corrected.

### D06 — `/compact` slash weak/absent feedback **[FIXED]**
- Fix: paint `/compact` row like `/refine`; result details use `success`/`severity`; emit result after `getMessages()`.
- Verified: “Compaction finished.” visible in TUI.

### D07 — `rlm` / `rlm.bash` not pre-bound **[FIXED]**
- Fix: kernel bootstrap `import rlm` + `from rlm.bash import bash`.
- Verified: `rlm in globals: True`, `bash in globals: True`.

### D08 — After Agents View resume, usage shows `0 (0%)` **[FIXED]**
- Fix: `switchSession` calls `refreshContextUsage()` after resume when occupancy snapshot is missing.

### D09 — Aborted tool remains `◇` open diamond in transcript after resume **[FIXED]**
- Fix: abort/turn interrupted close pending tools; resume synthesizes aborted toolResults for unpaired calls.

### D10 — Skills autocomplete tags bundled skills `#user` **[FIXED]**
- Fix: mapSkillSourceInfo maps system/bundled → source builtin (#builtin).

## Passed (spot)

- Boot, `/model`, `/effort`, `/system-prompt`
- `/goal <text>` sets goal + “Pursuing goal”
- `/refine …` schedules with note
- `/skill` typeahead lists skills
- Mermaid renders as ASCII flowchart in TUI
- Agents View via Left (empty editor) + Enter resume restores history
- Memory/refine path (prior session): per-session `session-artifacts/…/harness`

## Phase 2 verification (2026-09-16 cont.)

- D04 Esc mid-ipython sleep: single Operation aborted, cell shows aborted marker, Executing clears promptly; no footer flood.
- D03 interrupt/complete flood: gated workspace recap on turnSawFileMutation + flood guard; verified hello/abort/resume without 2297-files footer.
- D08 resume usage: occupancyToContextUsage maps totalTokens/contextWindowTokens; Agents View resume keeps ~11k (4%).
- D09 aborted tool: live + resume show aborted marker (not open diamond).
- D10 skills: /skill: typeahead shows #builtin for system skills.
- D05: Escape+Enter is not Alt+Enter (AGENTS.md corrected).

## Phase 1 expansion notes

- T28 global refine: 
efine.run(..., global_=True) schedules successfully (scheduled: True).
- **2026-09-17 refinement memory QA (isolated `%TEMP%\devo-refine-mem-qa`)**: T12/T13/T28 + resume all PASS — direct `create_memory`, post-turn `refine.run` apply to local harness, `global_=True` cross-session, `devo resume <sid>` restores local QA_MEM_DIRECT+QA_MEM_REFINE. Note: bare `DEVO_RESUME_SESSION_ID` env is stripped by `devo` launch; use `devo resume <id>`.
- T23 subagent: agent_observe imports (get/list/recent); full spawn/wait child not exercised this pass.
- T27 plan mode: added /plan + /build slash → SessionSettingsPatch.mode; verified Plan Mode refuses file create (**D11 fixed**).
- T23: agent_observe available; no Python gent spawn module on RLM path (child agents are discrete tool/host — deeper spawn smoke still open).
- T24 image: not exercised this pass.

## Skill QA campaign (2026-09-16 evening)

Manual TUI (`deepseek-flash`) per-skill results. attach-image already verified earlier.

| Skill | Result | Evidence |
|-------|--------|----------|
| agent-observe | pass | `list_agents()` → `keys=['agents','current']` n=1 |
| compact | pass | `status()` + `run()` → `scheduled: True` |
| refine | pass | `status()` + `run(...)` → `scheduled: True` |
| goal | pass | create → get → complete |
| edit | pass | rewrote probe file to `NEW_SKILL_QA_MARKER` |
| websearch | expected config gap | clear hosted/`web_search` + Serper setup guidance |
| rlm-heartbeat | pass | create/list/pause/delete job (`status` active→paused→stopped) |
| agent-message | pass (root limits) | parent → `no parent for this session`; missing role rejected |
| linear | expected NotEnabled | `list_tools()` → `/mcp login linear` |
| notion | expected NotEnabled | `list_tools()` → `/mcp login notion` |
| skill-creator | pass | SKILL.md readable from `.system` |
| prime-intellect | removed | deleted from bundled + `.system` (intentional) |
| attach-image | pass (prior) | vision attach + describe |

### Defects fixed this campaign

### D12 — linear/notion `ModuleNotFoundError: mcp` **[FIXED]**
- Cause: kernel Python lacked `mcp` SDK; error was opaque.
- Fix: clearer RuntimeError in `rlm/mcp_base.py`; installed `mcp` into user site-packages; after install skills raise `NotEnabled` until `/mcp login`.

### D13 — skill auth dir defaulted to `~/.prime/agent` **[FIXED]**
- Cause: `PRIME_AGENT_CODING_AGENT_DIR` only set when `DEVO_HOME` env present; websearch/mcp_base defaulted to `.prime/agent`.
- Fix: always inject Devo home into kernel env; prefer `DEVO_HOME` / `~/.devo` in `mcp_base` + `websearch`.

### D14 — path-typed image bubble order **[FIXED]** (earlier same evening)
- Bubble now shows `[image:path] describe it.` (images before text + spaced join).

### D15 — rlm-heartbeat host deny **[FIXED]**
- Cause: `kernel_host` denied all `rlm_heartbeat.*`.
- Fix: wire create/list/update/delete to `ScheduleStore` (labeled multi-heartbeat + bare `5m` intervals).

### D16 — agent-message family routing / receipt shape **[FIXED]**
- Cause: child-only route; no `deliveryStatus`; docs pointed at wrong roster APIs.
- Fix: nuclear-family send (parent/sibling/child + broadcast), `deliveryStatus`, docs → `agent_observe.list_agents`.

### D18 — `/heartbeat` crashed: stub `parseHeartbeatCommand` **[FIXED]**
- Cause: client `cron-jobs.ts` left `parseHeartbeatCommand` as empty stub → `command.type` threw.
- Fix: schedule capability stays server-owned — added Native `session/heartbeat/command` (parse+apply in `crates/server/src/heartbeat_command.rs` + `ScheduleStore`); client only strips `/heartbeat` and calls RPC. Verified: set → `schedules.json` + tray “1 heartbeat”.

## Python cell wait-budget QA (2026-09-17)

Harness: psmux + `target/debug/devo.exe` (default `~/.devo`, model `deepseek-flash`). Budget continue/background/cancel/renewal covered by unit tests with short `python_cell_first_wait_ms` (full 3min first wait not exercised live).

| Scenario | Result | Evidence |
|----------|--------|----------|
| Fast cell `print(1+1)` | pass | `✓ python · print(1+1) · 0ms` → `2`; no park/watch |
| Esc mid-turn (long sleep request) | pass | `Operation aborted · 7s`; prompt restored |
| Bash still reachable after abort | pass* | prompt idle; model retried `rlm.bash` imports (activity, not exact text) |
| continue_fg / cancel / background / max renewals | pass (unit) | `ipython_wait_budget_*` in `devo-core`; parse/clamp in `devo-tools` + `devo-server` |

\*Live LLM budget→continue/background needs `python_cell_first_wait_ms` ~10–15s via `session/metadata/update` (default 180s).
