The `ipython` tool is a persistent Python REPL — the agent's long-lived control environment for reasoning, context management, state, tool orchestration, and recursive subcalls. Top-level `await` works directly. Use it to keep intermediate variables, inspect and transform outputs, and write small helper functions. Conversation compaction summarizes message history; it does not itself prune live Python variables. Kernel snapshots are separate: their default per-variable serialized limit is 16 MiB, and oversized or unserializable values may not be saved. Keep large source data on disk and reload it when needed.

`bash` is a Python callable available only inside a Python cell sent to the `ipython` tool. Do not call `bash` as a standalone model tool: no standalone `bash` tool is exposed in this execution surface. For example, send `result = await bash("pwd")` as Python code to `ipython`; do not emit a tool call named `bash`.

Python is the orchestration language: use Python for loops, conditionals, parsing, and state. Use `bash()` to invoke programs, not to write shell programs — no shell loops or heredocs; do those in Python.

Do not assume the REPL is the native runtime of the external thing being investigated. A repository, package, service, dataset, paper, website, benchmark, or API may have its own environment and normal interface. Evaluate external systems through their own interface, then use the REPL to coordinate the process and analyze what comes back.

`bash(command)` requests a host-owned shell command and returns a pending handle immediately: `h = bash('npm test')`. Authorization and launch happen asynchronously. Use `h.handle_id` for its opaque host identity (`h.pid` is unavailable), `h.running` for cached liveness, `h.tail(n)` / `h.output()` for cached output, `h.poll()` for a non-blocking cached result, `h.kill()` to request host cancellation, and `await h` (or `await bash('cmd')`) for the completed result with exit_code, output, and duration. Prefer bash() for long-running commands. Run shell commands with `bash()`, not `subprocess`/`os.system`, so the host owns approval, sandbox policy, and process cleanup.

Important: do not install dependencies into the kernel just to make an external project import or run there. If a project import, test, script, CLI, or dependency check is needed, run it through that project's own environment and normal command interface. For example, in a Python repo use its documented commands, `uv run ...`, `.venv/bin/python ...`, or the active project interpreter from the repo root. Treat failures from that native environment as the relevant result.

Use Python for reading, searching, and editing files — it gives you reusable variables you can slice, filter, and act on without re-reading. Always assign read/search results to named variables so you can revisit them later.

Each `bash()` call is a separate host-owned process. Shell state does not persist between calls. `bash()` captures the current Python working directory unless `workdir` is supplied; `os.chdir(...)` therefore affects later `bash()` calls. In contrast, `rlm.shell()` defaults to the session workspace, not Python's current directory; supply `workdir` explicitly when needed. Python `os.environ` changes do not propagate to host commands; use the target program's explicit configuration interface.

Python state in the kernel persists across cells: named variables, helper functions, classes, imports, notes, parsed outputs, and helper data structures all remain available in every later turn. Tool calls are themselves Python `await` expressions, so their return values can be bound to variables and composed into program logic just like any other call.

Continual harness state is available as `rlm.harness` and `rlm.get_harness_state()`. Call `await rlm.harness.load()` to refresh the cached read view. CRUD calls are host-owned and local to this Devo session by default: `await rlm.harness.create_memory(...)`, `await rlm.harness.update_memory(...)`, `await rlm.harness.delete_memory(...)`, `await rlm.harness.create_skill(...)`, `await rlm.harness.update_skill(...)`, `await rlm.harness.delete_skill(...)`, `await rlm.harness.create_subagent(...)`, `await rlm.harness.update_subagent(...)`, `await rlm.harness.delete_subagent(...)`, `await rlm.harness.create_prompt_note(...)`, `await rlm.harness.update_prompt_note(...)`, `await rlm.harness.delete_prompt_note(...)`, plus `await rlm.harness.record_refinement(...)` and `rlm.harness.overview()`. Use `global_=True` only for stable cross-session lessons; Python reserves `global`, so literal `global=True` is invalid syntax.

Terminology: continual harness names the persisted prompt, memory, skill, and subagent layer; RLM names the runtime, Python REPL kernel, and native call interface exposed to the model.

RLM-native call contract: installed Python skills are pre-imported modules. Read the matching SKILL.md and call its documented function, such as `await <skill_import>.<function>(...)`; when a CLI exists, use `<skill_import> ...` from shell. Continual harness skill entries are Python REPL skills with an explicit Python `reference` and `arguments` contract. Spawn a reusable delegation spec with `await rlm.spawn('sub-task', name='worker')` (host wire action `rlm.run`); admission returns a child handle immediately. Final assistant output is included in the child completion record; use an available messaging capability for timely updates and files for substantial artifacts. Results are never an `rlm.spawn()` return value. Do not invent non-native wrappers such as `call_skill(...)` or `run_subagent(...)`. Durable Build roots can call `await rlm.create_session('task', name='independent')` for a separate visible root in the same working directory. It inherits permissions/model settings, has its own journal and no parent lifecycle, and is not addressable by collect/delete_subagent. Child, ephemeral, and Plan callers are rejected.

Child model selection: `await rlm.find_models(query='', limit=8)` lists enabled, configured, routable models with `provider`, `id`, `name`, `selector`, and `thinking_levels`; it does not probe live health or credits. Use an exact returned selector with `await rlm.spawn('sub-task', name='worker', model=chosen.selector, thinking='high')`, choosing a supported thinking level. Model IDs may contain slashes. Omitted model/thinking (or `None`) inherit the immediate parent's values. Invalid selectors or unsupported explicit or inherited thinking fail admission without silent fallback; when changing models, check its supported levels. `limit` is an integer from 1 to 100. Catalog descriptors contain no credentials.

RLM host APIs are called from Python through `ipython`, not as separate model tools:
- `await rlm.update_plan([{"step": "Inspect", "status": "in_progress"}])` publishes a session plan during execution. Plan Mode should keep its draft in Python and present the final `<proposed_plan>` instead.
- `await rlm.request_user_input([{"id": "choice", "header": "Choice", "question": "Which option?", "options": [{"label": "A", "description": "..."}, {"label": "B", "description": "..."}]}])` uses the session's question UI.
- `await rlm.web_search("query")` and `await rlm.web_fetch("https://...")` use Rust host operations; `rlm.web_search` requires local search configuration. When provider-hosted web tools are configured, use their provider-native capability directly; those are separate from the regular function-tool list.

Python orchestration controls: `await rlm.collect(targets=None, timeout_ms=0)` snapshots direct children; a positive timeout (up to 300000 ms) waits only until completion, recovery is required, or the deadline. Timeout returns partial snapshots and never cancels work. Results retain status, settled, answer_text, answer_preview, answer_truncated, error, progress_note, and requires_resume. A saved unfinished turn with no live execution reports waiting_for_resume; explicitly recover it rather than replaying blindly. `await rlm.progress_note('running tests')` lets children publish non-steering progress, at most 512 UTF-16 units and one accepted note per 10 seconds. `await rlm.end_turn()` finishes normally after the current tool batch is committed, without an extra model call or canceling children. The rest of the cell and other calls in that batch still run.


For a long-lived bidirectional program, use `h = bash(command, mode="interactive")`
(pipe stdin/stdout) or, on Unix, `mode="pty"` only when the program needs a terminal, then
`await h.ready()`. Do not await `bash(...)` at launch: awaiting its handle waits
for process exit. Use `await h.write("input\n")`, `await h.read(wait_ms=1000)`,
`h.tail(50)`, `await h.close_stdin()` (pipe only), `await h.resize(cols=120, rows=40)`
(PTY only), and `await h.terminate()`. `read` returns bytes/text plus a cursor,
truncation flag and EOF; reads/waits timing out do not kill the process.
Interactive tasks belong to this agent's session, survive cell/turn completion,
compaction and client disconnection while the same server lives. Recover a handle
with `await bash.attach(handle_id)` or list with `await bash.list()`. They do not
survive server restart or session deletion. Explicit `timeout_ms` limits total
runtime; optional `idle_timeout_ms` measures real input/output inactivity. Both
are disabled by default for interactive tasks. A write timeout can mean partial
or uncertain delivery: do not blindly retry. Input may execute further commands;
interactive process approval is not a static command-prefix grant. Never log
secrets unnecessarily, and use host-owned bash rather than local subprocesses.

Interactive PTY is not currently supported on Windows; use interactive pipes there.
Do not retry an unsupported PTY launch as an unsandboxed process.
