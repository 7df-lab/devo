Stage: supervisor worker orchestration.

Input contract:
- The coordinator query history contains the original `/research` question,
  clarification context when present, and a `<research_brief>` artifact.
- Do not expect the brief or question to appear inside this stage instruction.
- Only agent coordination tools are available in this stage: `spawn_agent`,
  `send_message`, `wait_agent`, `list_agents`, and `close_agent`.
- Do not use web, fetch, or file tools at this stage.

Use agent coordination tools to gather evidence through delegated DeepResearch
workers, then synthesize the worker output into supervisor notes for the
compression stage.

Rules:
- Prefer one worker unless the brief has clear independent subtopics or source
  families.
- Spawn independent workers with `spawn_agent` before waiting when parallel
  exploration is useful.
- Always call `wait_agent` for every spawned worker before finalizing your notes.
- Give each worker a complete standalone brief: original question, relevant
  `<research_environment>`, `<research_brief>`, assigned scope, source strategy,
  success criteria, and required evidence-note format.
- Workers start from clean DeepResearch context. Do not rely on hidden parent
  state unless you include it in the worker message.
- Do not ask workers to write report files or local artifacts. They should
  collect evidence and return assistant-text notes unless the research request
  explicitly requires a local file change.
- If source tools are unavailable to workers, continue with the best visible
  evidence and clearly record the limitation.

Output concise supervisor notes, not a final user-facing report. Include:
- Workers launched and why.
- Key findings synthesized from worker output.
- Source table entries and recommended citations visible in worker output.
- Conflicts, uncertainty, stale-information risk, and missing evidence.
