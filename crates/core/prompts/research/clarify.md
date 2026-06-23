Stage: clarification gate.

Input contract:
- The runtime context is in user-role messages, including
  `<research_environment>` and the original `/research` question as its own
  message.
- Do not expect the original question to appear inside this stage instruction.
- Do not use web tools at this stage.

Decide whether a clarifying question is required before research can start. Ask
only when the request is too ambiguous to produce a useful report.

Question policy:
- Strongly prefer using the existing `request_user_input` ask-question tool when
  clarification is required.
- Ask only questions that would materially change the research scope, confirm or
  lock an important assumption, or choose between meaningful tradeoffs.
- Do not ask for information already present in the research context.
- Do not ask questions that can be answered by non-mutating inspection of local
  context.
- If a reasonable default would produce a useful report, do not ask; continue
  with that default and make the assumed scope explicit in the next stage.
- If the request asks for current, latest, recent, or today-specific
  information, do not ask for a time range only because the request is current;
  the research workflow can use web tools after this stage.

Using `request_user_input`:
- Follow the research Language policy for all user-visible tool fields,
  including question text, option labels, option descriptions, and any
  free-form prompt.
- Ask one concise question unless multiple independent answers are truly needed.
- Offer only meaningful multiple-choice options; do not include filler choices
  that are obviously wrong or irrelevant.
- If one option is the recommended default, put it first and add
  `(Recommended)` at the end of the label.
- In rare cases where an unavoidable, important clarification cannot be
  expressed with meaningful multiple-choice options, ask a direct free-form
  question through the tool.
- Do not ask whether the user wants research to proceed.

If no clarification is needed, respond with one concise sentence confirming the
research scope. Do not return JSON.
