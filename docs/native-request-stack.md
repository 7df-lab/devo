# RPC dispatch stack budget

The Native dispatcher awaits many different handlers. Rust's inline async
state must accommodate the largest branch, even when the client requests a
small operation such as initialization or ping. On Windows `dev-fast`, the
incoming request futures occupied 101,664 and 101,600 bytes in callers. Nested
debug test futures could overflow the default thread stack. Increasing the
test stack hid this dependency and made local validation less convenient.

`handle_incoming_with_actions` heap-pins the selected async handler before
awaiting it. This keeps each handler's state out of the dispatch future. It
also separates handler polling from the dispatch state; boxing only the outer
dispatch future was insufficient in the observed Windows debug failure.
Request routing, awaiting order, response construction and ACP post-response
actions are unchanged. The public methods remain async functions.

This costs one heap allocation for a selected handler, including ACP routing.
Cheap readiness checks, client response resolution and inline ping responses
remain inline. It does not create a task, change cancellation ownership, or
introduce parallel execution.

`native_request_future_size` limits both public incoming futures to 2 KiB of
caller storage. It infers their types without constructing the oversized
baseline future on the test stack. The changed Windows futures measured
1,240 and 416 bytes. The test intentionally allows layout variation instead
of requiring those exact implementation sizes.

The goal pause/resume regression now uses ordinary Tokio test threads instead
of a custom 8 MiB thread. Goal, persistence, queued-interruption and ACP tests
exercise the dispatch behavior as well as the size guard. Windows integration
tests that launch a real kernel need an already provisioned QA home; otherwise
first-run UAC setup can exceed their short timeouts. That setup limitation is
separate from the request future's storage budget.

These measurements concern caller storage and local validation reliability.
They do not establish a runtime throughput or compilation-speed improvement.
Use the opt-in `dev-fast` profile and affected-package commands described in
[development checks](development-checks.md) for measured build-efficiency improvements.
