---
name: websearch
description: Search the web using the backend selected in Devo, when there is no model-facing web_search tool. Do not switch backends or use a third-party API directly.
---

# Web Search

- If the model-facing `web_search` tool is available, call it directly.
- Otherwise, use `await rlm.web_search(query)` for the session's selected MCP or legacy local backend. `websearch.run(query)` is a compatibility wrapper that returns text.
- If search is off, denied, unconfigured, or fails, report that outcome. Do not use raw HTTP, another MCP tool, or another backend as a fallback. When a setup or selection change is needed, point the user to `/web`.

Use the results to answer the question and cite the source URLs you relied on. Do not invent results or citations.

```python
print(await rlm.web_search("Rust official release notes"))
```
