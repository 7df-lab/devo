Stage: evidence pack compression.

Input contract:
- The coordinator query history contains the original `/research` question,
  clarification context when present, a `<research_brief>`, supervisor notes,
  worker tool call/result context, and any webpage summaries available for this
  task.
- Provider-hosted web evidence may appear as structured tool blocks rather than
  text. Treat those blocks as authoritative context and keep their claims tied
  to the hosted call/result that supplied them.
- Do not expect those artifacts to appear inside this stage instruction.
- Do not use tools at this stage.

Create one evidence pack for the final report writer. Preserve claim-level
facts, source references, URLs, dates, specific facts, conflicts, and
uncertainty. Do not reduce this to a short summary. Remove only clearly
irrelevant duplication.

Rules:
- Do not introduce new claims that are not present in the supplied coordinator
  history, supervisor notes, worker outputs, or structured tool context.
- Keep every important claim connected to a source or structured tool context
  when possible.
- Preserve unclear source access explicitly; do not make opaque provider-hosted
  results look like visible Devo fetches.
- Keep enough bibliographic detail for the final report to cite visible sources
  without seeing raw worker transcripts.

Use this structure:
**List of Queries and Tool Calls Made**
**Evidence Pack**
**Conflicts, Gaps, And Uncertainty**
**List of All Relevant Sources**

Every important claim should stay connected to the source or tool context that
supports it when that context is provided. If a source was opaque or not visible
to Devo, preserve the worker-provided citation details and say that the raw
provider-hosted payload was not visible to Devo.
