# Devo RLM built-in Python skills

Thin kernel-facing packages pre-imported by RLM bootstrap. Each package wraps
`rlm.host_request` for host-owned actions (compact / refine / goal).

## Bootstrap import list

Always pre-import (see `RLM_BOOTSTRAP_SKILL_IMPORTS` in `crates/core/src/rlm_prompts.rs`):

| Import | Package path | Status |
|---|---|---|
| `compact` | `compact/` | wired → `compact.status` / `compact.run` |
| `refine` | `refine/` | wired → `refine.status` / `refine.run` |
| `goal` | `goal/` | wired → `goal.get` / `goal.create` / `goal.complete` (create/complete pending full Native bridge) |

Kernel bootstrap also injects globals: `rlm`, `bash`, `mcp`, `rlm.harness` /
`get_harness_state`. Do **not** teach or assert `rlm.create_session`.
