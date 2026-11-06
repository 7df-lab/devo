# `devo-harness`

Host-side types, file IO, prompt digest, and refinement apply for Devo's
Continual Harness.

`devo-harness` is a library used by `devo-server`. It does not run an agent
loop, execute tools, or talk to the Python kernel directly. It owns the shared
harness state that the server injects into prompts and updates during
refinement.

The full refine planner lives in the server. This crate provides the stable
schema, the on-disk operations, and the apply path that the planner uses.

## What the Continual Harness is

The Continual Harness is the agent's persistent, prompt-visible guidance and
memory. It is not the agent loop. It answers questions such as:

- What durable fact, preference, or lesson should this session remember?
- Which reusable skill or subagent spec has the agent already defined?
- Which narrow prompt note corrects a repeated behavior?
- What changed during refinement, and why?

It stores four kinds of entries:

| Kind | Purpose |
|---|---|
| `prompt` | A narrow behavioral note added to the system prompt. |
| `memory` | A durable fact, preference, or lesson. |
| `skill` | A reusable Python skill definition with a Python `reference` and `arguments`. |
| `subagent` | A reusable delegation spec or task pattern. |

A `skill` entry is a definition. It does not grant tools, MCP access, or
sandbox permissions. A `subagent` entry is guidance. It does not spawn a child;
actual spawning still goes through the host's `rlm.spawn` path.

Entries are **local** to one session by default, or **global** across sessions.
A **refinement** is a small, evidence-backed edit to the harness. It can create,
update, or delete entries.

## State model

`src/state.rs` defines the shared schema.

`HarnessState` contains:

- `schema`: the schema version. The current version is `SCHEMA_VERSION = 1`.
- `entries`: a map from kind (`prompt`, `memory`, `skill`, `subagent`) to
  entry id to `HarnessEntry`.
- `refinements`: the refinement history stored inside the state file.

`HarnessEntry` contains:

| Field | Meaning |
|---|---|
| `id` | Stable id within its kind. |
| `kind` | `prompt`, `memory`, `skill`, or `subagent`. |
| `title` | Short human-readable label. |
| `content` | The actual guidance or definition text. |
| `path` | Optional grouping path. |
| `scope` | Optional `local` or `global`. |
| `reference` | Optional JSON contract for a Python skill. |
| `arguments` | Optional JSON argument schema or examples for a skill. |
| `metadata` | Free-form JSON metadata. |
| `source` | Who created the entry (`agent`, `refine`, etc.). |
| `created_at`, `updated_at` | UTC timestamps. |
| `version` | Version number maintained by the writer. |

A `RefinementEvent` records:

- `id`, `trigger`, and `changes`;
- `evidence` and `outcome` strings (often empty);
- `created_at`.

`SCHEMA_VERSION` is part of the file. A schema change must be coordinated with
the Python runtime; see [Relationship to the Python kernel](#relationship-to-the-python-kernel).

## On-disk layout

The state is plain JSON on disk.

```text
<session_dir>/harness/harness_state.json     # local state
<session_dir>/harness/refinements.jsonl      # host refinement audit log
<devo_home>/harness/harness_state.json       # global state
```

`HarnessState::file_path(session_dir)` returns the local state path.
`refinements_log_path(session_dir)` returns the JSONL path. The same module also
exposes the canonical kernel snapshot paths used by the server:
`<session_dir>/kernel.dill` and `<session_dir>/kernel.json`.

A simplified state file looks like this:

```json
{
  "schema": 1,
  "entries": {
    "prompt": {},
    "memory": {
      "prefer_targeted_tests": {
        "id": "prefer_targeted_tests",
        "kind": "memory",
        "title": "Prefer targeted tests",
        "content": "Run the smallest relevant test target before the full suite.",
        "path": "general",
        "scope": "local",
        "reference": {},
        "arguments": {},
        "metadata": {},
        "source": "agent",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "version": 1
      }
    },
    "skill": {},
    "subagent": {}
  },
  "refinements": []
}
```

`HarnessState::load` returns an empty default state when the file does not
exist. A corrupt or schema-invalid file returns
`HarnessStateError::Corrupt`; the crate does not silently wipe it.

`HarnessState::save_atomic` creates parent directories, writes pretty JSON to a
temporary file in the same directory, and renames it over the target. Readers
never see a partially written state file.

`HarnessState::merged_with_global` produces a copy with global entries added.
Session-local entries win on an id collision because they are more specific.

## Crate map

| Module | Role |
|---|---|
| `src/state.rs` | Schema types, default state, load, atomic save, local/global merge. |
| `src/digest.rs` | Format a state into prompt text; strip and reattach digest blocks. |
| `src/inject.rs` | `HarnessDigestInjector` for the server's prompt assembly. |
| `src/apply.rs` | Refine proposals, edits, drift checks, and atomic apply. |
| `src/refinements.rs` | `refinements.jsonl` records and snapshot path helpers. |
| `src/auto.rs` | Root-session auto-refine settings and turn-based decision. |
| `src/lib.rs` | Public exports. |

## Prompt digest

The harness is only useful if the model sees it. `HarnessDigestInjector` loads
the session state, optionally merges the global state, and formats a digest for
the next prompt.

The digest:

- is injected during cold prompt assembly, after skills and before the goal
  section;
- groups entries under `Prompt notes`, `Memories`, `Harness skills`, and
  `Subagent specs`;
- labels global entries with `(global)`;
- includes a short `await refine.run()` hint;
- is wrapped in the `[harness-digest]` / `<harness_state>` markers.

The marker constants in `src/digest.rs` must stay aligned with the server's
history stripper. During compaction, the host removes old digest blocks from
the text sent to the summarizer. A fresh digest is injected into the compacted
head; the crate provides `reattach_harness_digest` for callers that assemble
history as a list of texts. This keeps stale memories out of the summary and
gives the next turn the current state.

The relevant helpers are:

- `format_harness_state_for_prompt`: format a state for a prompt;
- `wrap_harness_digest`: wrap a body in the digest markers;
- `strip_harness_digests`: remove digest blocks from arbitrary text;
- `is_harness_digest_only`: detect a message that contains only a digest;
- `filter_texts_for_summarizer`: drop digest-only messages and strip embedded
  digests;
- `reattach_harness_digest`: put a fresh digest ahead of compacted history.

`HarnessDigestInjector::digest_for_session_dir` propagates a corrupt-state
error. The convenience methods `digest_or_empty` and
`digest_or_empty_with_global` omit the digest instead, so a bad file cannot
break prompt assembly.

## Refinement

A refinement is planned by the server and applied through this crate.

`src/apply.rs` defines:

- `RefineProposal`: an id, trigger, summary, evidence, expected outcome, and a
  list of edits.
- `RefineEdit`: `create`, `update`, or `delete`, a kind, an id, and optional
  `before` / `after` entries.
- `apply_proposal_re_read`: load the current state, validate the proposal
  against the on-disk state, apply the edits, append a `RefinementEvent`, and
  save atomically.

`before` is the baseline captured when the proposal was planned. If the current
entry no longer equals that baseline, `apply_proposal_re_read` returns
`ApplyError::Drift` instead of overwriting a concurrent edit. A create fails
with drift if the id already exists. The id `base_system_prompt` is immutable
and is always rejected.

The server records applied refinements in two places:

- a `RefinementEvent` inside `harness_state.json`;
- a `RefinementResultRecord` appended to
  `<session_dir>/harness/refinements.jsonl`.

The JSONL record includes the applied edits, the harness state path, and an
optional `rollback_of` and `scope`. `append_refinement` creates the directory,
appends one JSON object per line, and calls `sync_all`.

The full flow is:

```text
model calls refine.run()
  -> refine skill sends a host_request
  -> server schedules a pending refinement
  -> turn boundary: planner builds a RefineProposal
  -> apply_proposal_re_read re-reads the target state
  -> drift check, apply, save_atomic
  -> append_refinement writes refinements.jsonl
  -> next turn's digest includes the new state
```

This crate does not decide what to refine. The server planner does that, using
the session model with a heuristic fallback. This crate only accepts a proposal
and applies it safely.

## Auto-refine settings

`src/auto.rs` defines `AutoRefineSettings`:

| Field | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Enable turn-based auto-refine. |
| `turn_interval` | `25` | Successful user-visible turns between reviews. |
| `compact_enabled` | `true` | Enable the compact-review switch. |
| `compact_cooldown_ms` | `1_200_000` | Default 20-minute cooldown between compact reviews. |

`should_auto_refine` implements the turn-based decision. It returns `false` for
child sessions, goal-continuation turns, and disabled settings. The compact
fields are available to the server's scheduling code; the server owns the
actual schedule.

## Relationship to the Python kernel

The Python module `rlm.harness` in `crates/kernel/rlm-runtime` is a separate
implementation of the same JSON schema. It is not generated from this crate and
does not link to Rust. The two sides meet on disk.

- The RLM kernel exposes `rlm.harness` to cells and skills for direct CRUD.
- The Rust host reads the same file to build the prompt digest.
- The host applies refinements and writes the audit log.
- The kernel uses the same `schema: 1` shape and the same local/global paths.

The host passes the paths to the kernel through environment variables set in
`crates/core/src/tools/handlers/ipython/startup.rs`:

| Variable | Meaning |
|---|---|
| `RLM_SESSION_DIR` | Session artifact root. |
| `RLM_HARNESS_STATE_DIR` | Local harness directory. |
| `RLM_GLOBAL_HARNESS_STATE_DIR` | Global harness directory under Devo home. |
| `DEVO_CODING_AGENT_DIR` | Devo config directory, including `auth.json`. |

### Concurrent writers

The host and the kernel may write the same state file from different processes.
There is no cross-process lock; the protocol is optimistic.

- The kernel keeps a long-lived `HarnessState` and checks the file mtime before
  each local mutation. If another process changed the file, it reloads first.
- The host's `apply_proposal_re_read` re-reads immediately before applying and
  rejects per-entry drift.
- Both sides use atomic replacement, so readers see either the old file or the
  new file, never a torn one.

The two sides treat corruption differently on purpose:

- Rust's `HarnessState::load` rejects corrupt JSON. The digest injector either
  propagates that error or omits the digest; it never invents content.
- Python's loader treats an unreadable or corrupt file as empty so a bad file
  cannot kill the RLM kernel. The next kernel save rewrites a clean file.

If you change a field, update both implementations and bump `SCHEMA_VERSION`.

See [`crates/kernel/rlm-runtime/README.md`](../kernel/rlm-runtime/README.md)
for the Python side.

## Relationship to `devo-server`

`devo-server` depends on `devo-harness`. Important call sites:

- `crates/server/src/runtime/turn_exec/query.rs` injects the prompt digest at
  turn start.
- `crates/server/src/runtime/refine.rs` schedules, plans, and applies
  refinements.
- `crates/server/src/runtime/refine_planner.rs` builds the LLM proposal and
  falls back to the heuristic planner.
- `crates/core/src/history/harness_digest_strip.rs` uses matching digest
  marker constants when preparing compaction input.

The crate is deliberately small. It does not depend on session, model, or turn
types, and it does not talk to the Python process. That keeps the file format
and apply rules testable without a running server.

## Design rules

- **Schema is shared, not owned by one language.** Rust and Python mirror the
  same JSON shape.
- **Local is more specific than global.** On an id collision, the session entry
  wins in the merged digest.
- **Refinement is optimistic and drift-checked.** A proposal never overwrites
  an entry that changed after planning.
- **The base system prompt is immutable.** `base_system_prompt` is rejected by
  the apply path.
- **Atomic writes.** Temporary file plus rename; no partial reads.
- **Fail closed at the host boundary.** A corrupt state file is an error for
  the digest loader, not invented prompt content.
- **Keep prompt formatting stable.** Digest markers are a compatibility
  surface with compaction and history filtering.

## Development

Run the crate tests:

```bash
cargo test -p devo-harness
```

Check formatting and docs:

```bash
cargo fmt --check -p devo-harness
RUSTDOCFLAGS="-D warnings" cargo doc -p devo-harness --no-deps
```

The tests use `tempfile` and `pretty_assertions`; they do not need a server or
a Python interpreter.

## Where to start reading

| Order | File | Why |
|---|---|---|
| 1 | `src/state.rs` | The schema and the on-disk contract. |
| 2 | `src/digest.rs` | How state becomes prompt text and survives compaction. |
| 3 | `src/inject.rs` | The server-facing digest entry point. |
| 4 | `src/apply.rs` | Refine proposals, drift checks, and apply. |
| 5 | `src/refinements.rs` | The JSONL audit log and path helpers. |
| 6 | `src/auto.rs` | Root auto-refine settings. |
| 7 | `crates/server/src/runtime/refine.rs` | The host scheduling and apply pipeline. |
| 8 | `crates/server/src/runtime/refine_planner.rs` | LLM and heuristic planning. |
| 9 | `crates/kernel/rlm-runtime/src/rlm/harness.py` | The Python CRUD view of the same files. |

Related documents:

- [`crates/kernel/rlm-runtime/README.md`](../kernel/rlm-runtime/README.md) —
  the Python kernel and its `rlm.harness` API.
- [`crates/kernel/README.md`](../kernel/README.md) — the Rust host that starts
  the Python kernel.
- [`crates/server/src/runtime/refine.rs`](../server/src/runtime/refine.rs) —
  the host refine pipeline.
- [`crates/core/src/history/harness_digest_strip.rs`](../core/src/history/harness_digest_strip.rs)
  — digest filtering during compaction.

## Glossary

| Term | Meaning |
|---|---|
| Continual Harness | Persistent prompt-visible guidance and memory. |
| entry | One prompt note, memory, skill, or subagent spec. |
| local scope | State for one session. |
| global scope | State shared across sessions. |
| refinement | A small, evidence-backed edit to harness state. |
| proposal | A planned set of refinements, with evidence and expected outcome. |
| drift | An entry changed between planning and apply. |
| digest | The formatted harness state injected into a prompt. |
| base system prompt | The immutable prompt id that refinements cannot edit. |
