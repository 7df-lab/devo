---
name: edit
description: Replace an exact, unique string in an existing file. Use for targeted single-occurrence edits to files from the Python kernel instead of rewriting the whole file.
---

# Edit

Make a targeted edit to an existing file by replacing one exact, unique
occurrence of a string. `old_str` must appear exactly once in the file.

Call directly from the kernel:

    await edit.run(path="pkg/file.py", old_str=old, new_str=new)

Use exact old/new strings. If the text contains triple double quotes, use
triple single-quoted variables (`old = '''...'''`) or build `old`/`new` from
inspected file slices. Returns a short confirmation; raises if `old_str` is
missing or matches more than once (widen the snippet to make it unique).

If the `edit` CLI is installed in the host environment, invoke it through
host-mediated shell execution:

    result = await bash('edit --path pkg/file.py --old-str "..." --new-str "..."')

Use the Python API for complex strings to avoid shell quoting mistakes. Do
not use IPython `!` shell escapes to bypass host approval or sandbox policy.
