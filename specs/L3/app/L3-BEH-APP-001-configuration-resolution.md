---
artifact_id: L3-BEH-APP-001
revision: 3
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-27
---

# L3-BEH-APP-001 - Configuration Resolution And Persistence

## Purpose

Define implementation behavior for loading, validating, field-level merging, inspecting, and persisting the user-scoped and workspace-scoped `config.toml` files and the user-scoped-only `auth.json` file.

The `auth.json` credential file exists exclusively within the user configuration directory (`~/.devo` on macOS/Linux or `C:\Users\username\.devo` on Windows). The workspace configuration directory (`<workspace>/.devo`) never contains an `auth.json`.

The two `config.toml` sources are merged field by field. The user file provides the base. The workspace file overlays it. If the same specific field appears in both files, the workspace value wins. Fields present only in the user file remain effective.

## Source Design

- `L2-DES-APP-002` defines source precedence and persistence targets.
- `L2-DES-APP-005` defines the `config.toml` and `auth.json` schemas.
- `L2-DES-MODEL-001` defines provider and model binding semantics.
- `L2-DES-APP-004` defines diagnostics, privacy, and redaction expectations.

## Core Types

```rust
pub enum ConfigScope {
    User,
    Workspace { workspace_root: PathBuf },
}

pub struct ConfigInputPaths {
    pub user_config: ConfigFilePath,
    pub workspace_config: Option<ConfigFilePath>,
    pub user_auth: UserAuthPath,
}

pub struct ConfigFilePath {
    pub scope: ConfigScope,
    pub config_dir: PathBuf,
    pub config_path: PathBuf,
}

pub struct UserAuthPath {
    pub config_dir: PathBuf,  // ~/.devo or C:\Users\username\.devo
    pub auth_path: PathBuf,   // always <user-config-dir>/auth.json
}

pub struct LoadedConfigInputs {
    pub user_config: LoadedConfigFile,
    pub workspace_config: Option<LoadedConfigFile>,
    pub user_auth: LoadedUserAuth,
}

pub struct LoadedConfigFile {
    pub path: ConfigFilePath,
    pub config: Option<ConfigDocument>,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

pub struct LoadedUserAuth {
    pub path: UserAuthPath,
    pub document: Option<AuthDocument>, // missing file means empty credential set
    pub diagnostics: Vec<ConfigDiagnostic>,
}

pub struct ConfigurationResolution {
    pub effective: EffectiveConfig,
    pub user_auth: UserAuthStore,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

pub struct EffectiveConfig {
    pub defaults: EffectiveDefaults,
    pub providers: BTreeMap<ProviderId, EffectiveProvider>,
    pub model_bindings: BTreeMap<ModelBindingId, EffectiveModelBinding>,
    pub mcp_servers: BTreeMap<McpServerId, EffectiveMcpServer>,
    pub skill_roots: BTreeMap<SkillRootId, EffectiveSkillRoot>,
    pub tool_config: EffectiveToolConfig,
    pub workspace_instructions: EffectiveWorkspaceInstructionConfig,
    pub tui: EffectiveTuiConfig,
    pub logging: EffectiveLoggingConfig,
    pub telemetry: EffectiveTelemetryConfig,
    pub provenance: ConfigProvenance,
}

/// Plaintext credential material is retained only in this user-auth store and
/// must never appear in routine client projections.
pub struct UserAuthStore {
    pub path: UserAuthPath,
    pub credentials: BTreeMap<CredentialId, SecretCredential>,
    pub provenance: BTreeMap<CredentialId, AuthValueSource>,
}

pub struct ConfigProvenance {
    pub values: BTreeMap<ConfigPath, ConfigValueSource>,
    pub merged_records: BTreeMap<ConfigRecordPath, MergedRecordSource>,
    pub credential_refs: BTreeMap<ConfigPath, CredentialResolutionSource>,
}

pub struct ConfigValueSource {
    pub scope: ConfigScope,
    pub file: PathBuf,
    pub path: ConfigPath, // canonical leaf path, e.g. providers.openai.base_url
}

pub struct MergedRecordSource {
    pub record_path: ConfigRecordPath,
    pub identity_key: String,
    pub contributing_scopes: Vec<ConfigScope>,
    pub field_sources: BTreeMap<String, ConfigValueSource>,
}

pub struct AuthValueSource {
    pub file: PathBuf, // always <user-config-dir>/auth.json
    pub credential_id: CredentialId,
}

pub struct CredentialResolutionSource {
    pub credential_id: CredentialId,
    pub auth_source: AuthValueSource,
}

pub enum ConfigWriteTarget {
    UserConfig,
    WorkspaceConfig { workspace_root: PathBuf },
    UserAuth,
}
```

These types intentionally model `auth.json` separately from both `config.toml` sources. `ConfigScope` applies only to `config.toml` values and therefore has no auth variant. Any code path that needs credential material must go through `LoadedUserAuth` or `UserAuthStore`, which always point at the user configuration directory. Implementation may use concrete TOML and JSON libraries of choice, but the public behavior must preserve source identity, perform field-level merging, and avoid losing unrelated user-editable content.

## B1. Resolve Source Paths

- **Trigger**: CLI startup, server startup, `config/inspect`, onboarding persistence, `/model` persistence, or config refresh.
- **Preconditions**: The current workspace root is known or absent.
- **Algorithm / Flow**:
  1. Resolve the user-scoped configuration directory:
     - Windows: `C:\Users\username\.devo`
     - macOS and Linux: `~/.devo`
  2. Resolve user paths:
     - `config.toml`
     - `auth.json`
  3. If an active workspace directory exists, resolve the workspace config path:
     - `<workspace>/.devo/config.toml`
     (No `auth.json` at workspace scope — credentials live only in user scope.)
  4. Return sources in merge order for loading: user first, then workspace.
- **Postconditions**: The resolver never uses `~/.config/devo` or workspace `.dev` as the configured path for this schema. The `auth.json` path always resolves to the user directory only.
- **Errors**: Nonexistent files are not errors. Unreadable existing files produce source diagnostics.

## B2. Load And Parse Sources

- **Trigger**: Source paths are available.
- **Preconditions**: File access is available.
- **Algorithm / Flow**:
  1. Read each existing `config.toml` as UTF-8 text.
  2. Parse TOML into a syntax-preserving document when the implementation can do so; otherwise preserve unknown extension sections in a separate raw map.
  3. Read the user-scoped `auth.json` as UTF-8 JSON (workspace scope has no `auth.json`).
  4. Parse `auth.json` into `AuthDocument`.
  5. Attach `ConfigDiagnostic` records to the affected source instead of panicking.
- **Postconditions**: Missing sources are represented as empty sources. Malformed sources remain visible in diagnostics.
- **Errors**: Malformed higher-priority configuration blocks effective resolution of the affected setting; it must not silently fall back to lower-priority values for the same setting.

## B3. Validate Each Source Independently

- **Trigger**: A source has been parsed.
- **Preconditions**: Schema version is available or absent.
- **Algorithm / Flow**:
  1. Validate `schema_version`.
  2. Validate field types in known sections.
  3. Validate enabled keyed records contain required fields.
  4. Preserve unknown top-level extension sections under `[x.<namespace>]`.
  5. Produce diagnostics for unsupported fields under known sections.
  6. Validate the user-scoped `auth.json` credential shape without printing credential values.
- **Postconditions**: Source diagnostics identify path, table, field, severity, and recovery hint.
- **Errors**: Unsupported schema versions are fatal for that source.

## B4. Merge Sources Into EffectiveConfig

- **Trigger**: Source validation has completed.
- **Preconditions**: User and optional workspace sources are available.
- **Algorithm / Flow**:
  1. Start with the user-scoped `config.toml` as the base. Auth data comes exclusively from user-scoped `auth.json`.
  2. Merge the workspace-scoped `config.toml` on top.
  3. For scalar settings, a workspace value replaces the user value only when that scalar field is present in workspace config.
  4. For nested tables, merge recursively by table path and field name.
  5. For keyed records, the TOML table key is the record identity:
     - Same record key in workspace and user config merges field by field.
     - Same field in the same record uses the workspace value.
     - Fields present only in the user record survive unchanged.
     - `enabled = false` in workspace config disables the effective record even if user config enables it.
     - New workspace key adds a record.
     - User keys not mentioned by workspace config remain effective.
  6. Auth resolution uses only the user-scoped `auth.json`. Workspace config may reference credential ids that must exist in the user auth file.
  7. Record provenance for every effective leaf value and record identity. A merged record may therefore contain user-sourced and workspace-sourced fields.
- **Postconditions**: Effective configuration is deterministic and explainable. The merge never discards user-only fields or user-only records.
- **Errors**: A workspace credential reference that does not exist in user auth is an actionable error. Invalid workspace fields block fallback only for the affected field or record path; unrelated user fields remain effective.

## B5. Validate Effective Configuration

- **Trigger**: Effective configuration has been built.
- **Preconditions**: Built-in supported model catalog and known invocation methods are available.
- **Algorithm / Flow**:
  1. Validate defaults reference enabled effective records.
  2. Validate each provider has a valid base URL and credential reference.
  3. Validate each model binding references:
     - An enabled provider.
     - A supported `model_slug`.
     - A valid `invocation_method`.
     - A valid `default_reasoning_effort` when present.
  4. Validate `display_name` is present for enabled model bindings.
  5. Validate MCP server transport-specific fields.
  6. Validate skill root path syntax but defer expensive filesystem scans to skill discovery.
  7. Validate credential references against the user-scoped auth data.
- **Postconditions**: The server receives either an `EffectiveConfig` or a structured error set.
- **Errors**: Missing credentials, invalid higher-priority overrides, disabled providers, and invalid reasoning values are actionable configuration errors.

## B6. Inspect Configuration Safely

- **Trigger**: `config/inspect`, `/status`, model picker setup, provider diagnostics, or onboarding repair flow.
- **Preconditions**: Effective configuration or diagnostics are available.
- **Algorithm / Flow**:
  1. Build a client projection from `EffectiveConfig`.
  2. Include source scope, source path, enabled status, display names, provider names, model slugs, model names, invocation methods, and credential status.
  3. Exclude plaintext credential values.
  4. Include diagnostics and recovery hints.
  5. Mark whether values came from user scope or workspace scope when behavior differs.
- **Postconditions**: Clients can explain configuration without exposing secrets.

## B7. Plan Persistence Writes

- **Trigger**: Onboarding completes, `/model` creates or repairs a binding, a default is changed before first user message, graceful server exit persists reasoning effort, theme changes, permission policy changes, MCP setup changes, or skill root changes.
- **Preconditions**: The caller specifies a write intent and target scope.
- **Algorithm / Flow**:
  1. Convert the user action into a `ConfigWritePlan`.
  2. Resolve the target paths from B1.
  3. Classify changes:
     - `auth_only` (always writes to user scope)
     - `config_only` (writes to the caller-specified config scope)
     - `auth_then_config` (auth to user scope, config to caller-specified config scope)
  4. Validate that plaintext secret values appear only in the auth write candidate.
  5. Read the latest files again and rebase the intended change on the latest parsed documents.
  6. Validate the final candidate documents before writing.
- **Postconditions**: The write plan is explicit about target files, affected records, and whether credentials are involved.
- **Errors**: A plan that would write plaintext credentials to `config.toml` is rejected before disk write.

## B8. Commit Writes Atomically Per File

- **Trigger**: A valid write plan is ready.
- **Preconditions**: Parent directory can be created or already exists.
- **Algorithm / Flow**:
  1. Acquire locks for the files being changed:
     - user config or auth: `~/.devo/config.lock` or the Windows equivalent;
     - workspace config: `<workspace>/.devo/config.lock`.
  2. Create parent directories with user-only permissions where the platform supports them.
  3. If `auth.json` is changing (always in the user scope directory):
     - Write `auth.json.tmp` with restrictive file permissions.
     - Flush and rename it over `auth.json`.
  4. Revalidate the final `config.toml` candidate against the final user auth view.
  5. If `config.toml` is changing (in user or workspace scope):
     - Write `config.toml.tmp`.
     - Flush and rename it over `config.toml`.
  6. Release the lock.
  7. Emit `config/changed` and structured observability records.
- **Postconditions**: Each file replacement is atomic. Final `config.toml` must not reference a credential id absent from the final `auth.json`.
- **Recovery**: If auth succeeds but config fails, the result may contain an unused credential; that is safer than a config record referencing a missing credential. A later cleanup flow may remove unused credentials after user confirmation.
- **Errors**: On failure, report target path, scope, affected record id, and recovery hint without printing secret values.

## B9. Concurrent Edit Handling

- **Trigger**: A write plan is prepared while another process or editor may modify the files.
- **Preconditions**: Latest file hashes or timestamps are available.
- **Algorithm / Flow**:
  1. Capture base hash before editing.
  2. Re-read before commit.
  3. If unrelated edits can be merged by schema path, rebase and continue.
  4. If the same record or field changed externally, abort with `config_conflict`.
  5. Keep the user's file content unchanged on conflict.
- **Postconditions**: The program avoids overwriting user edits it cannot safely merge.

## B10. Required Tests

- Missing user and workspace config files produce empty effective configuration plus no fatal error.
- Missing user `auth.json` produces an empty auth document plus no fatal error until a credential reference requires it.
- Workspace scalar values override user scalar values only when the workspace field is present.
- Workspace keyed records merge field by field with user records of the same id.
- `enabled = false` in workspace config disables the user record with that id.
- Invalid workspace override blocks fallback for the same setting.
- `config/inspect` never returns plaintext credential values.
- Onboarding write creates `config.toml` in the caller-specified config scope and `auth.json` always in user scope.
- Workspace config writes target `<workspace>/.devo/config.toml`; the program never writes `<workspace>/.devo/auth.json`.
- `auth_then_config` never leaves final config (of any scope) referencing a credential id absent from user-scope `auth.json`.
- Conflicting concurrent edits abort without overwriting user changes.
- Unknown extension sections are preserved after unrelated writes.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| specifies | L2-DES-APP-002 | 2 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Implements source path resolution, precedence, effective config, inspection, and persistence target rules. |
| specifies | L2-DES-APP-005 | 2 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | Implements schema validation, safe projections, and atomic per-file writes for `config.toml` and `auth.json`. |
| related-to | L2-DES-MODEL-001 | 2 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | Validates provider and model binding records. |
| related-to | L2-DES-APP-004 | 1 | specs/L2/app/L2-DES-APP-004-observability-architecture.md | Emits configuration diagnostics and redacted change records. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-27 | Assistant | Initial | Initial L3 configuration resolution, validation, inspection, and persistence behavior. |
| 2 | 2026-05-27 | Assistant | Correction | Changed workspace config path to `<workspace>/.devo/config.toml`, made `auth.json` user-directory-only, and specified field-level workspace-over-user merge semantics. |
| 3 | 2026-05-27 | Assistant | Correction | Reshaped core types so user config/auth and optional workspace config are separate inputs, removed optional workspace auth modeling, and added field-level provenance for merged records. |
