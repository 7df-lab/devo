# devo-core

## Model data sources

There are exactly two sources: the live [models.dev](https://models.dev/api.json)
directory and user configuration. The server fetches and converts models.dev in
memory. No provider/model directory is embedded and no remote snapshot is written
to or restored from disk.

Public metadata is refreshed at startup and on demand through
`model/catalog/refresh`, respecting `catalog.refresh_interval_hours` unless forced.
Failed refreshes retain the same process's last successful directory. A fresh
offline process has only configured providers/models, not an offline public catalog.

User configuration is layered from `$DEVO_HOME/providers.json` and
`custom-providers.json`, workspace `.devo` equivalents, and explicit overrides.
Connections pin endpoint, wire API and model IDs; remote metadata cannot silently
change their request routing. Credentials stay separate in `auth.json`.
models.dev SDK packages without a supported adapter are omitted, not guessed.
Native OpenAI/Anthropic/Google endpoint defaults belong to their adapters, not a
separate catalog; explicit connect pins the resolved `base_url`.
Provider discovery, including authenticated Codex discovery, is explicit and
read-only. Results become user configuration only when explicitly saved.

## Offline or local source

```toml
[catalog]
offline = true
# Optional explicit local models.dev-format source, usable offline.
# source = "/path/to/models.dev-api.json"
refresh_on_startup = true
refresh_interval_hours = 1
```

Offline mode blocks catalog and provider-discovery network requests. An explicit
local source is an input, not an automatically maintained cache.

## Migration and tradeoffs

Existing configuration files are preserved. Sparse legacy connections that relied
on built-in endpoint defaults must be explicitly reconnected or configured with
an endpoint. Old `cache/models.dev-api.json`, `cache/models.dev-meta.json` and
provider snapshot files are ignored, not deleted. There is no build-time catalog
regeneration step. Live metadata and stable configured routes take priority over
first-run offline browsing.
