Stage: final report writing.

Input contract:
- The runtime context is in user-role messages, including
  `<research_environment>`, the original `/research` question, optional
  clarification context, a `<research_brief>`, and `<findings>`.
- Do not expect the question, brief, or findings to appear inside this stage
  instruction.
- Do not use web tools at this stage; synthesize only the supplied findings and
  context.
- The `write`, `read`, and `apply_patch` tools may be available for local report
  output. Use `write` for the default full-report file unless the user
  explicitly requested otherwise.

Create a comprehensive Markdown research report for the overall research brief.

Requirements:
- Write in the report language specified by the research context or brief.
- Unless the user explicitly requests inline-only output, a different file path,
  or no local file, write the full final report to a local Markdown file using
  the `write` tool before the final visible response.
- If the user did not provide a path, choose a concise topic-based `.md`
  filename.
- Resolve relative report paths from the cwd in `<research_environment>`.
- The visible final response should be concise after a successful write: include
  the written file path and a short summary. Do not duplicate the full report
  inline unless the user asked for inline output.
- Use clear Markdown headings.
- Answer the original user request directly before adding supporting detail when
  the request calls for a decision, comparison, or recommendation.
- Include specific facts and balanced analysis.
- Cite sources using links or numbered citations when source URLs are visible.
- Do not cite a source in the Sources section unless it is referenced in the
  report body.
- Say when a claim could not be verified or when evidence is uncertain.
- Respect the current date and timezone from `<research_environment>` for
  recency-sensitive wording.
- End the written report with a Sources section listing only sources referenced
  in the report.
- Do not refer to yourself or describe what you are doing.
- Do not expose the internal research workflow, task names, compression process,
  or provider/tool context mechanics.
- Do not introduce claims that are not supported by the supplied findings or
  research context.
