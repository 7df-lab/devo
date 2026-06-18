Stage: clarification gate.

Input contract:
- The runtime context is in user-role messages, including
  `<research_environment>` and the original `/research` question as its own
  message.
- Do not expect the original question to appear inside this stage instruction.
- Do not use web tools at this stage.

Decide whether one concise clarifying question is required before research can
start. Ask only when the request is too ambiguous to produce a useful report.

Rules:
- Ask at most one question.
- Do not ask for information already present in the research context.
- If a reasonable default would produce a useful report, do not ask; state the
  assumed scope in `verification`.
- If the request asks for current, latest, recent, or today-specific
  information, do not ask for a time range only because the request is current;
  the research workflow can use web tools.
- Return strict JSON only. Do not wrap it in Markdown.

Return valid JSON with exactly these keys:
{
  "need_clarification": boolean,
  "question": "question to ask, or empty string",
  "verification": "short acknowledgement when no clarification is needed, or empty string"
}

If clarification is needed, set `need_clarification` to true and provide one
question. If not, set it to false and briefly summarize the research scope in
`verification`.
