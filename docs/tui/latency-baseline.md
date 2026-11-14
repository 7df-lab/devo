# Real PTY latency baseline

The RPC stress tool measures server work, not the terminal that users interact
with. This small companion runs the **real Rust CLI → Node InteractiveMode →
Native server** in an isolated tmux/psmux session. It changes no product code.

```sh
cargo build -p devo-cli --bin devo --locked
cd apps/tui
npm ci --ignore-scripts --no-audit --no-fund
node --test scripts/tui-latency.test.mjs
node --import tsx --test scripts/tui-latency-input.test.ts
node scripts/tui-latency.mjs
```

Run on a machine allowed to create tmux sockets. Do not work around a sandbox
refusal. The existing CI tmux job runs it and uploads `tui-latency-<commit>` JSON,
including failed/partial runs. No latency value currently fails CI: only failure
to complete the scenario does. Collect comparable baselines before proposing a
performance budget.

## What is measured

All values are monotonic-clock elapsed milliseconds observed from outside the
product, at a fixed 100 × 30 terminal size:

| Metric | Start → observed end |
| --- | --- |
| `first_trust_screen_ms` | Launch request → default-No workspace trust screen |
| `accepted_to_editable_ms` | Send consent keys → initial Native session, chat chrome, and unsubmitted draft probe visibly rendered |
| `input_echo_ms` | Send literal draft probe → probe visibly rendered |
| `new_session_editable_ms` | Submit pretyped `/new` → distinct Native session, success notice, and new draft probe rendered |
| `resume_empty_editable_ms` | Submit pretyped `/resume <id>` → exact original session resumed, success notice, and new draft probe rendered |

This is an **offline, empty-session baseline**, not a history-load or model-turn
benchmark. Each sample starts a fresh process with a fresh home and an empty
workspace. The script explicitly accepts only that new workspace's trust prompt,
disables network catalog refresh and update checks via normal configuration,
reads no personal configuration/credentials, and never submits a prompt or calls
a model. Draft probes are cleared between phases; Native trace responses are
correlated to the current phase and exact resumed session. No mock TUI, internal
ready marker, preload, or replacement rendering path is used.

## Interpreting the report

JSON contains every completed raw sample, nearest-rank p50/p95/min/max, source revision,
CLI SHA-256, Node/tmux/OS/CPU information, and the polling interval. Defaults are
5 samples and a 25 ms polling interval; p95 of five samples is simply the maximum.
These are **upper-bound observations**, including tmux command startup, capture,
trace reads, and polling delay, not frame timings or sub-millisecond keystroke
latency. Do not subtract the interval or treat it as an error bound: capture and
scheduler overhead are additional. No warm-up samples are discarded. OS file
caches remain warm across runs; these are not cold-cache startup measurements.

Use the same machine, load, terminal size, Node/tmux versions, build profile and
polling interval to compare commits. Build the CLI from the tested checkout: a
binary compiled elsewhere can resolve that other checkout's TUI. The source SHA
and binary hash are provenance, not proof that arbitrary `DEVO_BIN` belongs to
that source. Debug and release numbers must not be mixed. CI runner variability
makes a single run unsuitable for a regression verdict.

Optional environment variables: `DEVO_BIN`, `DEVO_TMUX_BIN`,
`DEVO_BENCH_SAMPLES`, `DEVO_BENCH_POLL_MS`, `DEVO_BENCH_TIMEOUT_MS` (per observation,
default 60000), `DEVO_BENCH_OUTPUT` (default `tui-latency.json` in the current
directory). A failed run exits nonzero and does not emit a summary. Temporary
homes, traces, and the privately-owned tmux server are cleaned up. The report
contains no terminal transcript or credentials.

Large persisted history, populated session picker, streaming, scrolling and
subagent load are deliberately future scenarios. Use `devo-stress` for current
RPC/corpus measurements; do not present the empty-session baseline as coverage
of those workloads or as a comparison with another product.
