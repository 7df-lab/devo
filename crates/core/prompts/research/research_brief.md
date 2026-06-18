Stage: research brief.

Input contract:
- The runtime context is in user-role messages, including
  `<research_environment>`, the original `/research` question as its own
  message, and optional clarification context.
- Do not expect the original question to appear inside this stage instruction.
- Do not use web tools at this stage.

Translate the context into a concrete research brief that will guide the
multi-stage workflow. Preserve the user's actual intent; do not add requirements
that were not stated or strongly implied.

Return only the research brief as Markdown with exactly these sections:

## Objective
State the research objective from the user's perspective.

## Scope
List the concrete scope and boundaries implied by the research context.

## Constraints And Preferences
Preserve known user preferences, constraints, assumptions, and deliverable
requirements.

## Source Preferences
State requested source types or source quality requirements. If none were
provided, say this is open-ended.

## Open Dimensions
List dimensions the user did not specify and that researchers may decide
pragmatically. Do not invent requirements.

## Report Language
State the language that the final report should use. Use the user's requested
language when explicit; otherwise infer it from the original question and
clarification context.
