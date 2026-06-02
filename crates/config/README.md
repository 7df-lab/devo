# Config

This crate owns Devo's file-backed runtime configuration.

Shared serializable config contract types that are consumed by multiple crates
also live here. Runtime interpretation of the resolved config stays in the
consumer crates.

## Module Map

- `lib.rs` re-exports the public config surface.
- `app.rs` defines `AppConfig`, its defaults, config loading, merge behavior,
  validation, project config keys, and `AppConfigStore`.
- `server.rs` defines server transport and connection defaults.
- `logging.rs` defines logging and rolling file-log settings.
- `skills.rs` defines skill discovery settings.
- `error.rs` defines app and provider config error types.
- `provider.rs` re-exports provider config APIs and contains provider-focused
  tests.
- `provider/` contains provider config schema, TOML/JSON persistence, auth
  storage, provider resolution, and the provider config store.
- `tests.rs` contains app-config loader tests.

## Config Files

The user-level config file is `<DEVO_HOME>/config.toml`. `DEVO_HOME` defaults to
`~/.devo`; if the environment variable is set, it must point to an existing
directory.

When a workspace is known, the project-level config file is:

```text
<workspace>/.devo/config.toml
```

Provider credentials are stored separately in:

```text
<DEVO_HOME>/auth.json
```

`auth.json` stores secret values, while `config.toml` stores references to those
credentials.

## Load And Merge Order

`FileSystemAppConfigLoader` starts from `AppConfig::default()` and overlays
config in this order:

1. User config: `<DEVO_HOME>/config.toml`
2. Project config: `<workspace>/.devo/config.toml`
3. CLI overrides

Later layers win over earlier layers for overlapping fields. TOML tables are
merged recursively; non-table values replace the earlier value.

Provider-owned fields use `ProviderConfigSection::merge_overlay` while loading
user and project config, so project config can override specific provider fields
without clearing every omitted provider field from user config.

## App Defaults

`AppConfig::default()` currently sets:

- `summary_model = "UseTurnModel"`
- `server.listen = []`
- `server.max_connections = 32`
- `server.event_buffer_size = 1024`
- `server.idle_session_timeout_secs = 1800`
- `server.persist_ephemeral_sessions = false`
- `logging.level = "info"`
- `logging.json = false`
- `logging.redact_secrets_in_logs = true`
- `logging.file.directory = None`
- `logging.file.filename_prefix = "devo"`
- `logging.file.rotation = "Daily"`
- `logging.file.max_files = 14`
- `skills.enabled = true`
- `skills.user_roots = ["skills"]`
- `skills.workspace_roots = ["skills"]`
- `skills.watch_for_changes = true`
- `skills.bundled.enabled = true`
- `skills.include_instructions = true`
- `skills.config = []`
- `updates.enabled = true`
- `updates.check_on_startup = true`
- `updates.check_interval_hours = 24`
- `project_root_markers = [".git"]`
- `projects = {}`

## App Config Shape

Top-level app config fields include:

```toml
summary_model = "UseTurnModel" # or "UseAxiliaryModel"
project_root_markers = [".git"]

[server]
listen = []
max_connections = 32
event_buffer_size = 1024
idle_session_timeout_secs = 1800
persist_ephemeral_sessions = false

[logging]
level = "info"
json = false
redact_secrets_in_logs = true

[logging.file]
directory = "diagnostics"
filename_prefix = "devo"
rotation = "Daily" # Never, Minutely, Hourly, or Daily
max_files = 14

[skills]
enabled = true
user_roots = ["skills"]
workspace_roots = ["skills"]
watch_for_changes = true
include_instructions = true

[skills.bundled]
enabled = true

[[skills.config]]
path = "/path/to/skill/SKILL.md"
enabled = false

[[skills.config]]
name = "code-review"
enabled = true

[updates]
enabled = true
check_on_startup = true
check_interval_hours = 24

[projects."/path/to/project"]
permission_preset = "default" # read-only, default, auto-review, or full-access
```

`logging.file.directory` is optional. Relative logging directories resolve under
`DEVO_HOME`.

## Validation

`validate_app_config` rejects configs when:

- `server.listen` contains duplicate endpoints.
- `logging.file.max_files` is less than `1`.
- `logging.file.filename_prefix` is empty or whitespace.
- `updates.check_interval_hours` is less than `1`.
- `skills.user_roots` contains duplicate paths.
- `skills.workspace_roots` contains duplicate paths.
- `skills.config` entries include both `path` and `name`.
- `skills.config` entries include neither `path` nor `name`.
- `skills.config` name selectors are empty.

Provider-specific validation happens while resolving or mutating provider
config.

## Provider Config

Provider config is part of `config.toml` and is modeled by `ProviderConfigSection`.
The current provider schema uses provider vendor entries plus model bindings:

```toml
model = "gpt-5.4"
model_thinking_selection = "medium"
model_auto_compact_token_limit = 970000
model_context_window = 997500
disable_response_storage = true
preferred_auth_method = "apikey"

[defaults]
model_binding = "gpt54-main"

[providers.main]
enabled = true
name = "Main Provider"
base_url = "https://api.example.com/v1"
credential = "main_api_key"
wire_apis = ["openai_responses"]

[model_bindings.gpt54-main]
enabled = true
model_slug = "gpt-5.4"
provider = "main"
model_name = "gpt-5.4"
invocation_method = "openai_responses"
default_reasoning_effort = "medium"
```

Supported `wire_apis` and `invocation_method` values are:

- `openai_chat_completions`
- `openai_responses`
- `anthropic_messages`

The default `invocation_method` is `openai_chat_completions` when a model
binding omits it.

`preferred_auth_method` accepts `apikey` and `api_key`; it serializes as
`apikey`.

Legacy `[model_providers]` fields still deserialize into `ProviderConfigSection`,
but the provider resolver does not use legacy-only config to produce runtime
provider settings.

## Provider Credentials

`auth.json` is modeled by `UserAuthConfigFile`. Example
`<DEVO_HOME>/auth.json`:

```json
{
  "version": 1,
  "credentials": {
    "main_api_key": {
      "kind": "api_key",
      "value": "secret-value"
    }
  }
}
```

The credential id, such as `main_api_key`, is referenced from provider config
with `credential = "main_api_key"`.

Only `api_key` credentials are currently supported. Reading `auth.json` fails if
the schema version is unsupported or a credential value is empty. Missing
`auth.json` is treated as an empty credential file.

## Provider Resolution

`resolve_provider_settings_from_config_and_auth` chooses the active model
binding in this order:

1. `[defaults].model_binding`, when it points to an existing binding.
2. The top-level `model`, when it matches a binding's `model_slug` or
   `model_name`.
3. The first enabled model binding.

After a binding is selected, resolution requires:

- The binding's `provider` exists in `[providers]`.
- The provider is enabled.
- The binding is enabled.
- If the provider lists `wire_apis`, the binding's `invocation_method` is in
  that list.
- If the provider references a credential, that credential exists in
  `auth.json`.

The resolved runtime settings contain the provider id, wire API, final model
name, optional base URL, optional API key, model limits, thinking selection,
response-storage flag, and preferred auth method.

## Writing Provider Config

Provider writes use atomic file replacement. They preserve unrelated TOML in
`config.toml` and only overlay provider-owned keys.

`AppConfigStore::upsert_provider_vendor` writes provider vendors and model
bindings to the user config. Project config may still override resolved
settings, but onboarding and provider management persist shared provider setup in
the user-level `config.toml`. The upsert rejects provider vendors with an empty
`wire_apis` list and reloads the effective app config after a successful write.
