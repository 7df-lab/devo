# Native prompt ownership and runtime contracts

The Native TUI, Desktop, and CLI use the Rust prompt pipeline. The base system
prompt is built by `crates/core/src/rlm_prompts.rs`; session context adds catalog
custom guidance and the active Build/Plan template. Query-time web capability
instructions are appended separately. Skills and workspace/environment context
are message prefixes, not additional copies of the base system prompt.

## Policy ownership

- The RLM base owns language clarity, root progress, delegation mechanics, and
  host-mediated shell/approval guidance. User-facing prose follows the requested
  language or the primary language of the latest user message, including plans
  and goals. The system and compatibility language prefix share one source text.
  Short, concrete prose does not imply an English-only requirement.
- Build owns independent execution, material assumptions, milestones, and
  verification. Assumptions cannot substitute for required approval.
- Plan owns the no-mutation boundary, exploration, material questions, and the
  decision-complete `<proposed_plan>` contract. User execution requests alone do
  not switch the active mode. Tests may write untracked build artifacts but must
  not change tracked files.

## Agent identity

The server derives `AgentPromptIdentity` from durable session ancestry for each
query and `session/systemPrompt/read`. `QueryOptions` carries it into the shared
system builder. Children and grandchildren get their own depth and parent path,
including after restart; ordinary forks keep root identity. It is not stored in
or inherited with the locked `SessionContext`, so old persisted contexts cannot
force a child back to root doctrine. The original root-only builder remains
available for callers that do not have agent ancestry.

## Verified runtime details

- `rlm.spawn` returns admission metadata, not the child's answer. Native
  `kernel_host.rs` rejects model/thinking overrides; children inherit both.
  Final assistant text is retained as completion detail in `runtime/agents.rs`.
  Explicit `agent_message` updates remain useful for timely delivery; substantial
  artifacts can be shared through files. Child `refine.run` is rejected.
- Conversation compaction (`core/src/history/compaction.rs`) summarizes message
  history. It does not invoke Python namespace pruning. Kernel snapshots are a
  separate mechanism: `kernel/rlm-runtime/src/rlm/repl.py` has a 16 MiB default
  per-variable serialized cap and may omit unserializable or oversized values.
  Namespace pruning requires the separate `prune_oversized` option, which defaults
  to false. Do not promise all Python objects survive snapshot/restore.
- The Python `bash()` wrapper captures Python's cwd unless `workdir` is supplied.
  `rlm.shell()` omits that field by default, so the host uses the session workspace.
  Both use host-owned authorization and execution. Python environment changes do
  not propagate to host commands. The default shell resolver uses Bash on Unix
  and PowerShell on Windows, not necessarily the interactive shell in context.

The bundled compact skill states the same snapshot boundary. The edit skill
calls the imported module’s `edit.run(...)` API and, when its CLI is installed,
uses host-mediated `bash()` rather than IPython shell escapes.

## Compatibility surfaces

The TypeScript builders in `apps/tui/lib/coding-agent/src/core/system-prompt.ts`
and `core/prompts/rlm.ts` are legacy, not the active Native request assembly.
Their exports remain intact: extension types and the extension runner still
consume `BuildSystemPromptOptions`. Remove them only with a consumer migration,
not as a prompt deduplication change. Likewise, the Rust XML catalog formatter is
retained for compatibility; active skill metadata uses the shared Markdown
renderer. `default_base_instructions.txt` is the legacy-default comparison
baseline, not a second default system prompt to delete.

Tests in `collaboration_mode_prompts.rs` and `rlm_prompts.rs` pin mode boundaries,
language guidance, actual runtime contracts, and root/child differences. Changes
to wording should retain these behavioral assertions rather than snapshotting a
large prompt verbatim.
