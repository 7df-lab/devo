# Changelog

## Unreleased

- `/traces` is wired to Devo-local native protocol traces (status/on/off/preview) instead of an upload flow; the TUI transport now records traces under `DEVO_HOME/traces` when `DEVO_PROTOCOL_TRACE` is set.
- `/quit` is `Quit devo`; upstream self-update checks and the `/update` builtin are disabled.

## 0.1.39

- Devo InteractiveMode TUI on Native protocol (session lifecycle, Agents View, slash commands).
- Brand splash uses the DEVO wordmark; product version shown as `devo v0.1.39`.
- Subagent sessions resume from their own rollouts in Agents View.
- Agents View delete uses opaque session ids (not fake session-file paths).
- `/login` excludes hosted-provider branding; multi-provider selector only.
