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

First-slice smoke: T1 idle, T2 prompt, T3 interrupt. Agents View is off.
