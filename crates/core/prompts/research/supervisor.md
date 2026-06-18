Stage: supervisor task plan.

Input contract:
- The runtime context is in user-role messages, including the original
  `/research` question and a `<research_brief>` artifact.
- Do not expect the brief or question to appear inside this stage instruction.
- Do not use web tools at this stage.

Create a bounded research plan for server-scheduled researcher tasks.

Rules:
- Prefer one task unless the brief has clear independent subtopics.
- Create at least one task and at most {{ max_tasks }} tasks.
- Each task must be a single, standalone topic with enough detail for a
  researcher that cannot see other tasks.
- Include source strategy and success criteria that make the expected evidence
  clear.
- Avoid overlap between tasks.
- Stop planning once the tasks can answer the brief.
- Return strict JSON only. Do not wrap it in Markdown.

Return valid JSON:
{
  "tasks": [
    {
      "title": "short task title",
      "research_topic": "detailed standalone research instructions",
      "purpose": "which part of the brief this task answers",
      "source_strategy": "what source types or search strategy the researcher should prioritize",
      "success_criteria": "how to know this task has enough evidence"
    }
  ]
}
