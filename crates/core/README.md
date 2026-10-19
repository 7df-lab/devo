# devo-core

## Built-in provider catalog (`providers.json`)

`crates/core/providers.json` is the git-tracked built-in provider/model
directory. It is embedded at compile time via `include_str!` in
`src/model_catalog.rs` and is always available offline.

### Runtime refresh from models.dev

At server startup Devo can download
[https://models.dev/api.json](https://models.dev/api.json) into:

```text
$DEVO_HOME/cache/models.dev-api.json   # raw dump (source of truth)
$DEVO_HOME/cache/models.dev-meta.json  # fetch time + counts (refresh interval)
```

On catalog load, Devo **converts `models.dev-api.json` in memory** into a
provider overlay and merges it onto the embedded builtin. There is no separate
converted cache file.

Effective merge order:

1. Embedded `crates/core/providers.json`
2. Converted overlay from cached `models.dev-api.json`
3. User `providers.json` (builtin Connection overlays)
4. User `custom-providers.json` (user-owned providers/models)
5. Workspace `.devo/providers.json` / `.devo/custom-providers.json`
6. CLI / `[model.*]` overrides

### Why not only `models.dev-api.json`?

That **is** the durable network artifact. Devo does not keep a second converted
JSON for correctness — conversion runs when the catalog loads. `meta.json` only
records when the dump was fetched so refresh can honor
`catalog.refresh_interval_hours`.

### DeepSeek V4.1 Flash naming

Under the official `deepseek` provider, models.dev uses id `deepseek-flash`
with display name "DeepSeek V4.1 Flash". Other gateways may list a different
model id such as `deepseek-v4.1-flash`. Those are separate catalog entries —
Devo persists whichever `provider/model` id the user selects, with no remap.

### Offline / no-internet configuration

In `$DEVO_HOME/config.toml`:

```toml
[catalog]
# Never fetch over the network.
offline = true
# Optional local dump of models.dev api.json (still works when offline).
source = "C:/path/to/models.dev-api.json"
refresh_on_startup = true
refresh_interval_hours = 24
```

### User-owned providers/models

Put custom providers in `$DEVO_HOME/custom-providers.json` (same JSON shape as
`providers.json`). Builtin Connection credentials/overlays stay in
`providers.json`.

### Build-time regeneration

```bash
node --experimental-strip-types scripts/import-pi-ai-catalog.mjs --refresh-snapshot
```
