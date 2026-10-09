# Native model catalog refresh

Desktop and TUI model directories use `model/list`, `provider/list`, and
`credential/list`. Credential results contain metadata and masks only.
Desktop preserves Native preference selection values, including custom
connection and variant paths.

`model/catalog/refresh` accepts `{ "policy": "ifStale" }` or
`{ "policy": "force" }`. It returns a `status` of `updated`, `cached`,
`offline`, or `failed`; a failed result also includes `message`.

The server owns fetching, conversion, and persistence. It uses the existing
`[catalog].source` (default `https://models.dev/api.json`) and refresh interval
(default 24 hours). `ifStale` honors that interval. `force` bypasses it.
Both honor offline mode; local catalog dumps still work offline.
The startup refresh setting does not disable explicit runtime refresh.

A valid remote dump updates the cache under `$DEVO_HOME/cache`, after conversion
succeeds. Failed downloads or invalid dumps retain the last valid catalog.
Fresh cache checks require the same source and an existing complete dump.
Refresh serializes catalog writes independently of session actor locks.
An update invalidates workspace contexts; list RPCs read the live cached catalog
without restarting the server. User provider and model overrides retain priority.

This method is Native only. External adapter wire surfaces remain unchanged.

## Connected provider discovery

Settings → Providers → Manage uses Native `provider/discover`, separately from
models.dev directory refresh. Sparse connections inherit their effective endpoint
and protocol; discovery resolves API keys and refreshable OAuth access through
the same credential resolver used for turns. ChatGPT uses the Codex backend
`/models?client_version=99.99.99`, an OAuth bearer and the account header, and
parses remote model slugs, context windows, modalities, and reasoning levels.
Remote entries marked `visibility: hide` remain disabled in Ready to use.
Discovery preserves the effective provider name and inherited model protocols.
The version follows OpenAI's own bundled catalog fetch:
https://github.com/openai/codex/blob/main/.github/workflows/rust-release-prepare.yml

Desktop permits 60 seconds for catalog and provider discovery RPCs so its normal
10-second request budget does not expire before the server's HTTP timeout.
Failures remain visible in the provider dialog, which supports retry.
