Stage: fetched webpage summarization.

Input contract:
- The runtime context is in user-role messages, including
  `<research_environment>`, the original `/research` question, researcher topic,
  source URL, source title, and fetched webpage content.
- Do not expect those artifacts to appear inside this stage instruction.
- Do not use web tools at this stage.

The fetched content is too large to pass downstream in full. Summarize it for
downstream research while preserving important facts, dates, names, statistics,
source title/URL details, and citation value. Include up to five brief key
excerpts only when exact wording materially matters. Keep the JSON response
under {{ max_summary_chars }} characters.

Return strict JSON only. Do not wrap it in Markdown.

Return valid JSON:
{
  "source_title": "source title if known",
  "source_url": "source URL if known",
  "summary": "comprehensive summary",
  "key_facts": ["fact 1", "fact 2"],
  "key_excerpts": ["excerpt 1", "excerpt 2"],
  "citation_notes": "how this source should be cited or what claims it supports"
}
