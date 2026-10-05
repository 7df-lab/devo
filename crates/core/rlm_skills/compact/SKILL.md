---
name: compact
description: Check context usage and compact the conversation from the Python REPL. Use when context is filling up, so the session is summarized and context is freed for the next stretch of work.
---

# Compact

Compaction replaces older conversation history with a dense summary, freeing
context so long-running work can continue. The host owns compaction (same path
as `/compact`); this skill is the kernel-side interface.

```python
await compact.status()
await compact.run()
await compact.run("keep the failing test names and the migration checklist")
```

## API

- `await compact.status()` — context usage: `tokens`, `context_window`, `percent`, `scheduled`.
- `await compact.run(instructions=None)` — schedule compaction for turn end.

## Rules

- Compaction never runs mid-cell; it runs when the current turn ends, and the
  turn then stays ended — you are not resumed automatically. Before
  scheduling, record the state the summary must carry (plan, handles, file
  paths, the concrete next step).
- The Python kernel persists through compaction (variables remain).
- Check `await compact.status()` when unsure.
