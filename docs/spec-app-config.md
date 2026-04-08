# ClawCodeRust Detailed Specification: App Config

## Background and Goals

The current spec set defines model config, safety config fragments, and server config fragments, but it does not yet define the top-level application configuration contract that ties them together. This specification defines the app-level configuration surface for `clawcr`.

Primary goals:

- define a single source of truth for user-level and project-level runtime settings
- separate app configuration from model catalog data
- make cross-cutting settings such as summary-model selection explicit
- provide deterministic merge, override, and validation rules

## Scope

In scope:

- top-level config file locations and formats
- merge order across built-in, user, and project scopes
- cross-cutting runtime settings
- summary-model selection
- config validation and reload behavior

Out of scope:

- provider-specific credential formats
- per-session runtime persistence files
- UI-only preferences that do not affect runtime behavior

## Module Responsibilities and Boundaries

`clawcr-cli` owns:

- locating config files
- loading raw config documents
- applying CLI/session override layers
- reporting validation errors to the user

`clawcr-core::config` owns:

- typed config schema
- merge and precedence rules
- normalized runtime config construction
- layer metadata and field-origin tracking
- source-aware diagnostics with file/range reporting

Subsystem crates consume normalized config only:

- `clawcr-core::model` consumes model selection and default model slugs
- `clawcr-core::context` consumes compaction and summary-model settings
- `clawcr-core::conversation` consumes session-title policy and title-model settings
- `clawcr-safety` consumes redaction, sandbox, and approval defaults
- `clawcr-tools` consumes runtime enablement, timeout, truncation, and search defaults
- `clawcr-server` consumes listener and transport defaults

## File Locations and Formats

Required user-level config path:

```text
~/.clawcr/config.toml
```

Optional project-level override:

```text
<workspace>/.clawcr/config.toml
```

Rules:

- TOML is the canonical app-config format
- missing project config is normal
- missing user config falls back to built-in defaults
- model catalog remains a separate file and is not inlined into app config
- project config discovery may walk upward using configured project-root markers; default marker set includes `.git`

## Merge and Precedence Rules

Merge order:

1. built-in defaults compiled into the binary
2. user-level app config
3. project-level app config
4. CLI or session flag override layer
5. per-turn API overrides where explicitly allowed

Rules:

- scalar values override by nearest scope
- TOML tables merge recursively by key with nearest-scope override
- lists are replace-by-value unless otherwise specified
- secrets must not be logged during merge or validation
- invalid project config must not silently discard user-level config; it must fail loading with a scoped error
- disabled layers may remain visible in metadata but must not affect effective config

## Core Data Structures

```rust
pub struct AppConfig {
    pub default_model: Option<String>,
    pub summary_model: SummaryModelSelection,
    pub context: ContextConfig,
    pub conversation: ConversationConfig,
    pub safety: SafetyConfig,
    pub tools: ToolRuntimeConfig,
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub project_root_markers: Vec<String>,
}
```

```rust
pub struct ContextConfig {
    pub preserve_recent_turns: u32,
    pub auto_compact_percent: Option<u8>,
    pub manual_compaction_enabled: bool,
    pub snapshot_backend: SnapshotBackendMode,
}
```

```rust
pub struct ConversationConfig {
    pub session_titles: SessionTitleConfig,
}
```

```rust
pub struct SafetyConfig {
    pub policy_model: PolicyModelSelection,
}
```

```rust
pub struct ToolRuntimeConfig {
    pub enabled_tools: Vec<String>,
    pub shell: ShellToolConfig,
    pub file_search: FileSearchToolConfig,
    pub max_parallel_read_tools: u16,
}

pub struct ShellToolConfig {
    pub default_timeout_ms: u64,
    pub max_timeout_ms: u64,
    pub stream_output: bool,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
}

pub struct FileSearchToolConfig {
    pub prefer_rg: bool,
    pub max_results: u32,
    pub max_preview_bytes: usize,
}
```

```rust
pub enum SummaryModelSelection {
    UseTurnModel,
    UseConfiguredModel { model_slug: String },
}
```

```rust
pub enum SnapshotBackendMode {
    JsonOnly,
    PreferGitGhostCommit,
    RequireGitGhostCommit,
}
```

```rust
pub struct SessionTitleConfig {
    pub mode: SessionTitleMode,
    pub generate_async: bool,
    pub generation_model: TitleModelSelection,
    pub max_title_chars: u16,
}

pub enum SessionTitleMode {
    ExplicitOnly,
    DeriveThenGenerate,
}

pub enum TitleModelSelection {
    UseTurnModel,
    UseConfiguredModel { model_slug: String },
}
```

```rust
pub enum PolicyModelSelection {
    UseTurnModel,
    UseConfiguredModel { model_slug: String },
}
```

```rust
pub struct LoggingConfig {
    pub level: String,
    pub json: bool,
    pub redact_secrets_in_logs: bool,
    pub file: LoggingFileConfig,
}

pub struct LoggingFileConfig {
    pub directory: Option<PathBuf>,
    pub filename_prefix: String,
    pub rotation: LogRotation,
    pub max_files: usize,
}

pub enum LogRotation {
    Never,
    Minutely,
    Hourly,
    Daily,
}
```

```rust
pub struct ConfigLayerEntry {
    pub source: ConfigSource,
    pub version: String,
    pub disabled_reason: Option<String>,
}
```

```rust
pub enum ConfigSource {
    BuiltIn,
    User { file: PathBuf },
    Project { dot_clawcr_folder: PathBuf },
    CliOverrides,
}
```

## Interface Definitions

```rust
pub trait AppConfigLoader {
    fn load(&self, workspace_root: Option<&Path>) -> Result<AppConfig, AppConfigError>;
}
```

```rust
pub trait AppConfigLayerLoader {
    fn load_layers(
        &self,
        workspace_root: Option<&Path>,
    ) -> Result<Vec<ConfigLayerEntry>, AppConfigError>;
}
```

```rust
pub trait AppConfigResolver {
    fn resolve_summary_model<'a>(
        &'a self,
        app_config: &'a AppConfig,
        turn_model: &'a ModelConfig,
        catalog: &'a dyn ModelCatalog,
    ) -> Result<&'a ModelConfig, AppConfigError>;

    fn resolve_title_model<'a>(
        &'a self,
        app_config: &'a AppConfig,
        turn_model: &'a ModelConfig,
        catalog: &'a dyn ModelCatalog,
    ) -> Result<&'a ModelConfig, AppConfigError>;

    fn resolve_policy_model<'a>(
        &'a self,
        app_config: &'a AppConfig,
        turn_model: &'a ModelConfig,
        catalog: &'a dyn ModelCatalog,
    ) -> Result<&'a ModelConfig, AppConfigError>;
}
```

```rust
pub struct ConfigDiagnostic {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub message: String,
}
```

## Summary Model Contract

Summary generation must use an explicit app-level setting, not an implicit runtime heuristic.

Rules:

- `UseTurnModel` means compaction uses the same resolved model as the turn currently executing
- `UseConfiguredModel { model_slug }` means compaction resolves a separate model by slug
- if the configured summary model is missing, config validation fails
- if the configured summary model lacks text input/output capability required for summarization, config validation fails
- summary-model selection is session-agnostic unless a future per-session config layer is added

Rationale:

- this keeps cost/performance tuning configurable without hard-coding a “dedicated summarizer” policy into the architecture
- it allows local/offline or cheaper remote models to be used for compaction when desired

## Session Title Model Contract

Session-title generation must be independently configurable from summary generation.

Rules:

- `SessionTitleMode::ExplicitOnly` disables all automatic title derivation and title-generation jobs
- `SessionTitleMode::DeriveThenGenerate` enables deterministic provisional derivation after the first completed exchange and optional asynchronous model-based finalization
- `generate_async = false` means the runtime may derive a provisional title but must not start a model-based title-upgrade job
- `TitleModelSelection::UseTurnModel` means final automatic title generation uses the same resolved model as the completed turn
- `TitleModelSelection::UseConfiguredModel { model_slug }` means final automatic title generation resolves a separate model by slug
- if the configured title model is missing, config validation fails
- if the configured title model lacks text generation capability, config validation fails
- title generation remains best-effort; config controls eligibility and model choice, not whether provider failure aborts the session

Rationale:

- Claude Code's provisional-title-then-upgrade behavior is useful, but the final model choice should remain operator-configurable
- this keeps session naming cost and latency tunable without coupling it to compaction policy

## Safety Policy Model Contract

Model-guided safety policy must use an explicit app-level model-selection setting.

Rules:

- `PolicyModelSelection::UseTurnModel` means policy classification uses the same resolved model as the active turn
- `PolicyModelSelection::UseConfiguredModel { model_slug }` means policy classification resolves a separate model by slug
- if the configured policy model is missing, config validation fails
- if the configured policy model lacks the capabilities required for policy classification, config validation fails
- policy-model selection only affects model-guided policy evaluation and must not bypass deterministic enforcement in `clawcr-safety`

Rationale:

- this keeps the cost, latency, and trust boundary of model-guided policy configurable
- it avoids hard-coding either “always active model” or “always smaller classifier” into the architecture

## State Transitions and Lifecycle

Config lifecycle:

1. locate user-level config
2. locate optional project-level config
3. parse source layers as TOML documents
4. append CLI/session override layer when present
5. merge layers in precedence order
6. validate cross-field references
7. produce immutable normalized `AppConfig`
8. pass normalized config into runtime bootstrap

Hot reload:

- out of scope for the first milestone
- if later added, reload must be atomic and versioned

## Snapshot Backend Contract

Context compaction snapshotting must also be controlled by app config.

Rules:

- `JsonOnly` means compaction persists JSON snapshot metadata only
- `PreferGitGhostCommit` means compaction persists JSON metadata and additionally attempts a detached git ghost snapshot when available
- `RequireGitGhostCommit` means compaction must fail if a git ghost snapshot cannot be captured in an eligible repository
- JSON metadata remains required even when git ghost snapshots are preferred or required

## Validation Rules

Required validations:

- referenced default model must exist if set
- referenced summary model must exist if `UseConfiguredModel` is used
- referenced title model must exist if `UseConfiguredModel` is used
- referenced policy model must exist if `UseConfiguredModel` is used
- `auto_compact_percent`, if set, must be between 1 and 99
- `preserve_recent_turns` must be at least 1
- `max_title_chars` must be between 20 and 120
- `tools.enabled_tools` entries must refer to known tool names
- `tools.shell.default_timeout_ms` must be less than or equal to `tools.shell.max_timeout_ms`
- `tools.file_search.max_results` must be at least 1
- server listener config must not define duplicate identical endpoints
- `logging.file.max_files` must be at least 1
- `logging.file.filename_prefix` must not be empty
- `RequireGitGhostCommit` must be rejected when running in environments that explicitly disable git integration

Cross-module validations:

- app config may reference model slugs defined in the model catalog, but must not redefine model capabilities here
- app config may narrow safety defaults, but must not bypass hard safety invariants defined by the runtime

Diagnostics requirements:

- parse and typed-schema errors must be surfaced with source file, line, and column when available
- when merged config fails schema validation, the loader should prefer reporting the first concrete per-file error rather than only a merged synthetic error
- field-origin metadata should remain available for debug or API introspection

## Persistence, Caching, and IO

- config is read at process startup
- parsed config may be cached in memory for process lifetime
- no config-derived runtime state is written back into `config.toml`
- runtime journals and approval decisions remain separate from config
- the loader may retain raw layer text transiently for diagnostic reporting, but normalized runtime config should not depend on raw text after load

## Observability

Logs must include:

- config source scope used for each override
- resolved default model slug
- resolved summary-model mode
- resolved session-title mode
- resolved session-title generation model mode
- resolved safety policy model mode
- resolved snapshot backend mode
- whether project config was loaded
- active project-root markers
- durable rolling file sink details including directory, rotation mode, and retention count

Logging persistence:

- default durable log directory is `~/.clawcr/logs`
- relative `logging.file.directory` values resolve under `CLAWCR_HOME`
- file logging uses rolling appenders with bounded retention
- tracing diagnostics are file-only; stderr is reserved for user-facing UI or command output
- the bootstrap should keep sink composition extensible so future telemetry exporters can be added without replacing local file logs

Metrics:

- `config.load.count`
- `config.load.failure.count`
- `config.validate.failure.count`

## Security and Edge Cases

- config parsing errors must not leak secret values in diagnostics
- unknown fields should fail closed in strict mode and warn in non-strict mode
- project config must not silently weaken security defaults without being visible in logs
- absent config files are not errors

## Testing Strategy and Acceptance Criteria

Required tests:

- built-in plus user plus project merge behavior
- CLI dotted-path override application
- project-root marker parsing
- summary-model resolution for both enum variants
- session-title model resolution for both enum variants
- safety policy model resolution for both enum variants
- explicit-only session-title mode disables automatic generation
- snapshot-backend mode validation
- missing referenced model validation
- invalid numeric threshold validation
- config diagnostics redaction

Acceptance criteria:

- runtime startup produces one normalized `AppConfig`
- context compaction can resolve its summary model from config without ad hoc branching
- conversation runtime can resolve its title-generation policy and model without hard-coded defaults
- safety runtime can resolve its policy-classification model without hard-coded defaults
- project overrides can change summary-model selection without modifying model catalog files

## Dependencies With Other Modules

- Language Model provides the catalog used to resolve configured model slugs
- Context Management consumes summary-model selection and compaction defaults
- Conversation consumes session-title policy and title-model selection
- Safety consumes policy defaults, policy-model selection, and redaction toggles
- Tools consumes runtime enablement and execution limits
- Server API consumes transport defaults

## Open Questions and Assumptions

Assumptions:

- app config and model catalog remain separate files
- TOML is preferred for app config because it is easier for humans to edit than large JSON documents

Open questions:

- Whether there should be a machine-generated `config.lock` later for resolved defaults and migrations
- Whether API clients should be allowed to override summary-model selection per session
- Whether API clients should be allowed to override session-title generation policy per session
