# Third-party runtimes (MIT)

Do not publish or depend on the npm registry for `@earendil-works` packages.

## RLM Python runtime

Source: Prime Agent `prime-agent-runtime` (MIT).

Path: `crates/kernel/rlm-runtime/src` (the directory that contains the `rlm` package).
Override with `DEVO_RLM_RUNTIME_SRC`.

## TUI stack

The product InteractiveMode TUI lives in `apps/tui`. UI libraries are in
`apps/tui/lib/{tui,ai,agent,coding-agent}`.
