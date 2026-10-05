---
name: websearch
description: Search the web. Prefer Devo hosted web_search (provider-hosted tool or local Exa/Tavily). Fall back to Serper only when hosted search is unavailable. Use when the user asks to search the web and no web_search tool is already available.
---

# Web Search

Prefer Devo's built-in search. Use this skill only when you cannot call a
model-facing `web_search` tool directly.

## Preference order

1. **Provider-hosted or local `web_search` tool** — if your tool list includes
   `web_search`, call that tool. Do not route through this skill.
2. **This skill** — tries host `web.search` (local Exa / Tavily when configured),
   then Serper if a key is saved.
3. **If nothing is configured** — tell the user how to enable search (below).
   Do not invent results.

## Enable hosted search (preferred)

- **Provider-hosted:** set `[tools.web_search] mode = "provider"` and use a
  model on Anthropic Messages or OpenAI Responses that supports hosted search.
- **Local Exa / Tavily:** set `[tools.web_search] mode = "local"` and configure
  a provider under `[tools.web_search.local_providers.<id>]` with
  `kind = "exa"` or `kind = "tavily"` (credentials via `/login` / auth.json).

## Serper fallback (third party)

Only when hosted search is not available:

1. Get a free API key at https://serper.dev
2. In Devo, run `/login` → **MCP Connections** → **Serper (web search)** and
   paste the key.

Do not ask the user to set environment variables.

Optional overrides:

- `DEVO_WEBSEARCH_TIMEOUT` — HTTP timeout in seconds (default 45).
- `DEVO_WEBSEARCH_NUM_RESULTS` — organic results to return (default 5).

## Usage

```python
print(await websearch.run("latest Devo release"))
```
