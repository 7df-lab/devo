Stage: research brief.

Input contract:
- The coordinator query history contains `<research_environment>`, the original
  `/research` question as its own user-role message, optional clarification
  tool results, and optional normalized `<clarification_context>` blocks.
- Use the current coordinator context; do not expect the original question or
  clarification answers to appear inside this stage instruction.
- Do not use tools at this stage.

Translate the coordinator context into a concrete research brief that will guide
supervisor-worker orchestration. Preserve the user's actual intent; do not add
requirements that were not stated or strongly implied.

The brief is the explicit handoff to later stages. If clarification answers or
reasonable default assumptions shaped the scope, record them in the brief rather
than relying on hidden prior conversation.

Return only the research brief as Markdown with exactly these sections:

## Objective
State the research objective from the user's perspective.

## Scope
List the concrete scope and boundaries implied by the research context.

## Constraints And Preferences
Preserve known user preferences, constraints, assumptions, clarification
answers, and deliverable requirements.

## Source Preferences
State requested source types or source quality requirements. If none were
provided, say this is open-ended.

## Open Dimensions
List dimensions the user did not specify and that researchers may decide
pragmatically. Do not invent requirements.

## Worker Decomposition Hints
Name independent subtopics or source families only when the brief naturally
separates into parallel worker assignments. Otherwise say one worker is likely
enough.

## Report Language
State the language that the final report should use. Use the user's requested
language when explicit; otherwise infer it from the original question and
clarification context.
