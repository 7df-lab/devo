# Devo product TUI (`apps/tui`)

InteractiveMode + Native `AgentConnection`. There is no `vendor/` tree.

## How it connects

```
devo CLI  →  tsx apps/tui/src/index.ts
                 │
                 ├─ host.ts                 InteractiveMode + AgentsViewMode (Native-only)
                 ├─ native-agent-connection.ts   AgentConnection over Native JSON-RPC
                 └─ lib/coding-agent        UI widgets, slash chrome, Agents View
                      lib/tui               terminal toolkit
                      lib/ai                model catalog / provider APIs
                      lib/agent             agent-core types used by the UI
```

RLM Python runtime lives in `crates/kernel/rlm-runtime`. Bundled skills live in
`crates/skills/assets/bundled`. Third-party notice: `THIRD-PARTY.md`.

## Layout

- UI libraries: `lib/{tui,ai,agent,coding-agent}`
- Host / adapter: `src/` (`NativeAgentConnection` emits InteractiveMode `session_event` shapes only)
- Launch: `devo` CLI → `node tsx apps/tui/src/index.ts` with `DEVO_SERVER_BIN`

## Setup

```bash
cd apps/tui && npm install --ignore-scripts
```

## Run

```bash
# via CLI (TTY required)
cargo run -p devo-cli

# direct
cd apps/tui && npm start
```

Config/auth: `DEVO_CODING_AGENT_DIR` / `PRIME_AGENT_CODING_AGENT_DIR` → `~/.devo` (never `~/.prime`).

## Traces

Native protocol traffic (TUI ↔ server NDJSON) can be recorded for debugging.

```bash
DEVO_PROTOCOL_TRACE=1 npm start                      # enable at launch (1/true)
DEVO_PROTOCOL_TRACE=1 DEVO_PROTOCOL_TRACE_FILE=/tmp/my-trace.ndjsonl npm start
```

Files land in `DEVO_HOME/traces/protocol-<pid>-<UTC>.ndjsonl` (`~/.devo/traces` by default), with the
same record shape the desktop logger writes (`timestamp`, `direction`, `kind`, `id`, `method`,
`payload`) so one reader can consume traces from any Devo host.

In-session control via `/traces`:

- `/traces` or `/traces status` — enabled state, active file, trace file count
- `/traces on` / `/traces off` — start/stop recording at runtime (`on` creates a new file)
- `/traces preview [count]` — last records (`count` defaults to 20, capped at 50)

Devo traces are local-only: there is no upload/login path. `/traces` is host-owned — the vendored
upload-oriented builtin is disabled through `excludedBuiltinCommands` (`src/host.ts`) and Devo serves
the command from `src/native-agent-connection.ts` over the `src/native-traffic-log.ts` sink.

## Tests

```bash
cd apps/tui && npm test
npm run test:tmux -- T1
```

Smoke scenarios: `T1` startup, `T2` completed model turn, `T3` interruption,
`goal` set/pause, `bash` shell output, `agents` agent manager, `schedule`
heartbeat status, and `heartbeat` scheduled delivery/clear.

The shared Node runner uses tmux (Unix) or psmux (Windows), an explicit repository
cwd, a unique session, and an isolated temporary Devo home. It copies local
provider configuration/auth for model scenarios, skips onboarding, and removes
the test session and home in `finally`. It never reuses or kills an existing
named session. No product shortcuts are changed: `agents` uses Left to open the
manager; Ctrl+O continues to change conversation detail.

Startup requires successful Native session creation and visible chat chrome.
Model and interrupt checks require matching Native turn outcomes, without
asserting exact model text. Heartbeat delivery requires a Native user item,
which avoids passing on the echoed scheduling command. Failures exit nonzero
and print the last captured pane.

Overrides: `DEVO_BIN` (or `DEVO_SERVER_BIN`), `DEVO_TMUX_BIN`,
`DEVO_TUI_TEST_SESSION` (must not exist), and `DEVO_TUI_TEST_TIMEOUT_MS`
(per-outcome timeout, default 90000). Build the CLI before running the harness.
