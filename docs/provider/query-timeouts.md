# Model query timeouts

The agent query loop uses a fixed five-minute (300-second) deadline for obtaining
the first normalized model event, including stream creation, and for each wait for
the next normalized model event.
This applies to OpenAI Chat Completions, OpenAI Responses and Anthropic adapters,
including servers that connect successfully but never return headers or only send
SSE keepalive comments. The existing TCP/TLS connection deadline remains 30 seconds.

Each model event resets the idle deadline. A continuously producing response has
no overall duration limit. Time spent handling an event locally is not charged
against the next model-event wait. These deadlines are not currently configurable.
Non-streaming SDK calls outside the agent query loop are not covered by this change.

A deadline creates a structured provider timeout. Before useful output, the query
loop applies its existing transient retry policy (at most five retries, with
250 ms, 500 ms, 1 s, 2 s and 4 s delays). After text, reasoning, or a tool-call start
has been observed, the attempt fails without automatic replay. This prevents
repeating partially displayed output or provider-hosted tool side effects.
The timeout does not manufacture complete tool arguments or execute partial calls.
