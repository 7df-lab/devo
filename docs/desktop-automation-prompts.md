# Desktop automation prompt delivery

Desktop automation runs submit their run guidance and configured task together as
ordinary user input. They do not replace or append to the server-owned system
prompt. The previous `system` argument to the compatibility SDK was unsupported
and silently discarded.

The executor builds two text parts: automation run guidance first, then the
configured task unchanged. `session.promptAsync` joins them into Native
`session/queue/push` input. The server normalizes that input and uses its shared
turn admission path, starting immediately if idle or queuing if busy. The guidance
therefore follows the same history, queue, and resume behavior as the task. It is
not a persistent session setting or an instruction for other sessions; a resumed
conversation retains the original run's input as history.

Only the memory file path is included. The agent reads memory with its normal
tools and permissions when needed, rather than the Desktop process eagerly
embedding arbitrary previous-run content as instructions. The run guidance
explicitly treats memory as untrusted reference data, not authorization, and
requires reporting permission blockers instead of bypassing them. Automation
name and path metadata are JSON-quoted to preserve line boundaries.

Regression tests cover prompt construction and the actual SDK-to-Native queue
request, including exact task text and guidance order. No Native protocol or
server-owned system prompt schema changes are required.
