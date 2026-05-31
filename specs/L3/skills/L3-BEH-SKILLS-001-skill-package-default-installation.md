---
artifact_id: L3-BEH-SKILLS-001
revision: 1
status: Draft
active_baseline: no
---

# L3-BEH-SKILLS-001 — Skill Package Definitions and Default Installation

## Purpose

Define the `skills` crate contract for skill package data structures, `SKILL.md` parsing, package validation, bundled default skills, and idempotent installation of default skills. Runtime catalog policy, activation, trust, instruction precedence, and context insertion are owned by core/server L3 behavior.

## Source Design

L2-DES-SKILLS-001, L3-DES-ARCH-001

## 1. Skill Crate Boundary

The `skills` crate owns package mechanics only:

- skill package type definitions;
- `SKILL.md` frontmatter and body parsing;
- package layout validation and diagnostics;
- safe supporting-resource path resolution;
- bundled default skill manifests and embedded package bytes;
- first-run and version-update installation of default skills into the user skill root.

The `skills` crate must not decide whether a skill is active for a session, inject instructions into model context, grant permissions, execute scripts, call tools, or write session JSONL records.

## 2. Package Data Model

```rust
pub struct SkillPackage {
    pub package_id: SkillPackageId,
    pub root: SkillPackageRoot,
    pub definition: SkillDefinition,
    pub resources: Vec<SkillResourceRef>,
    pub content_hash: Sha256Digest,
    pub diagnostics: Vec<SkillDiagnostic>,
}

pub struct SkillDefinition {
    pub name: SkillName,
    pub description: String,
    pub version: Option<String>,
    pub enabled: Option<bool>,
    pub tags: Vec<String>,
    pub compatibility: Option<SkillCompatibility>,
    pub allowed_tools: Vec<String>,
    pub instruction_body: String,
    pub frontmatter_format: FrontmatterFormat,
}

pub enum SkillSourceKind {
    BuiltInDefault,
    User,
    Workspace,
    Plugin,
    ExternalPackage,
}

pub struct SkillResourceRef {
    pub relative_path: Utf8PathBuf,
    pub resource_kind: SkillResourceKind,
    pub byte_len: Option<u64>,
}

pub enum SkillDiagnostic {
    MissingEntrypoint,
    UnreadableEntrypoint { reason: String },
    InvalidFrontmatter { reason: String },
    MissingRequiredField { field: &'static str },
    InvalidName { value: String },
    DescriptionTooLong { bytes: usize },
    BodyTooLarge { bytes: usize },
    ResourcePathEscapesPackage { path: String },
    UnsupportedResourceType { path: String },
}
```

`SkillPackageId` is a stable local identity derived from source kind, normalized package name, and either the canonical package root or bundled default id. It is not a user-facing display name.

## 3. `SKILL.md` Parsing

Parsing rules:

1. Read only the package entrypoint `SKILL.md` during normal discovery.
2. Require UTF-8.
3. Parse a `---` delimited frontmatter block at the top of the file.
4. Accept YAML or TOML-like key/value frontmatter only when the parser can identify it unambiguously.
5. Require `name` and `description` for normal discoverability.
6. Treat content after frontmatter as `instruction_body`.
7. Bound frontmatter, body, and total entrypoint bytes. Oversized packages receive diagnostics and are unavailable for automatic activation.
8. Do not read `references/`, `scripts/`, `assets/`, or other supporting files during entrypoint parsing.

Name validation should be deterministic and portable. Recommended normal form is lowercase ASCII letters, digits, `_`, and `-`, with length 1 to 64. Invalid names remain visible in diagnostics but do not become normal catalog entries.

## 4. Package Validation

Validation rules:

1. Package root must be canonicalizable.
2. Entrypoint must be located directly under the package root.
3. Supporting resource references must remain inside the package root after path normalization.
4. Symlinks inside skill packages must not escape the package root unless a later trusted-source policy explicitly allows it.
5. `allowed_tools` is advisory metadata only. It does not authorize tool use.
6. Scripts are package resources, not executable privileges.
7. Validation must return diagnostics rather than panicking on malformed packages.

The crate may enumerate supporting resource paths for display and activation metadata, but it must not read supporting file contents unless a caller explicitly requests that resource through normal controlled tool behavior.

## 5. Bundled Default Skill Model

Default skills are shipped with the program as read-only package assets represented by:

```rust
pub struct DefaultSkillBundle {
    pub bundle_version: String,
    pub skills: Vec<DefaultSkillAsset>,
}

pub struct DefaultSkillAsset {
    pub default_skill_id: String,
    pub package_name: SkillName,
    pub package_version: Option<String>,
    pub content_hash: Sha256Digest,
    pub files: Vec<EmbeddedSkillFile>,
}

pub struct EmbeddedSkillFile {
    pub relative_path: Utf8PathBuf,
    pub bytes: &'static [u8],
    pub executable: bool,
}
```

Default skill assets are trusted as shipped program assets, but their instructions still have normal skill precedence and cannot override user intent, safety, permissions, approval, or project instructions.

## 6. Default Skill Installation

Default skills must be installed idempotently so users can inspect and customize them.

```rust
pub struct DefaultSkillInstaller {
    pub bundle: DefaultSkillBundle,
}

pub struct DefaultSkillInstallOptions {
    pub user_skill_root: PathBuf, // default: ~/.devo/skills/
    pub mode: DefaultSkillInstallMode,
}

pub enum DefaultSkillInstallMode {
    InstallMissingAndUpdateManaged,
    InstallMissingOnly,
    DryRun,
}

pub struct DefaultSkillInstallReport {
    pub installed: Vec<SkillName>,
    pub updated: Vec<SkillName>,
    pub skipped_user_modified: Vec<SkillName>,
    pub skipped_conflict: Vec<SkillName>,
    pub failed: Vec<DefaultSkillInstallFailure>,
}
```

Installation algorithm:

1. Acquire a user-scope skill-install lock.
2. Ensure the user skill root exists with user-only permissions where the OS supports them.
3. For each bundled default skill:
   - target package path is `<user_skill_root>/<package_name>/`;
   - managed metadata path is `<target>/.devo/default-skill.toml`;
   - if target does not exist, copy the bundled package to a temporary directory and atomically rename it into place;
   - if target has managed metadata and current package hash equals the recorded installed hash, update it when the bundled hash changed;
   - if target has managed metadata but current package hash differs from the recorded installed hash, treat it as user-modified and do not overwrite;
   - if target exists without managed metadata, treat it as a user package conflict and do not overwrite.
4. Write or update managed metadata after successful copy:

```toml
default_skill_id = "..."
bundle_version = "..."
installed_hash = "sha256:..."
installed_at = "..."
last_synced_at = "..."
managed_by = "devo"
```

5. Return a report with all actions and diagnostics.

Installation failure must not fail program startup. It must produce diagnostics that the core catalog can surface through `skills.list`, `skills.refresh`, or status inspection.

## 7. Catalog Handoff to Core

The `skills` crate returns parsed package data and install reports. Core uses those outputs to build the runtime `SkillCatalog`:

- source precedence and duplicate handling;
- trust state;
- enablement state;
- model-visible catalog projection;
- user-explicit and model-selected activation;
- durable activation records;
- context integration.

Installed default skills should be classified as `BuiltInDefault` unless the user modifies the package after installation. User-modified managed defaults remain visible, but core may classify them separately as user-modified defaults for trust and update diagnostics.

## 8. Required Tests

Implementation must include tests or fixtures for:

1. Valid `SKILL.md` with required fields parses into `SkillDefinition`.
2. Missing `name` or `description` produces diagnostics and no normal catalog entry.
3. Supporting paths that escape the package root are rejected.
4. Discovery parsing does not read supporting file contents.
5. Default installer copies missing bundled skills into `~/.devo/skills/`.
6. Default installer updates a managed package whose hash still matches the previous installed hash.
7. Default installer does not overwrite user-modified managed defaults.
8. Default installer does not overwrite an existing user package with the same name and no managed metadata.
9. Dry run reports intended actions without writing files.
10. Installation failure reports diagnostics without aborting startup.

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-SKILLS-001 | specified-by |
| L3-DES-ARCH-001 | specified-by |

## Implementation Placement Guidance

- The crate should be named `devo-skills`.
- Bundled default skills should be included as build-time assets or a packaged resource directory with content hashes generated at build time.
- The installer should use atomic directory replacement where supported. If atomic replacement is unavailable, use a staging directory plus fsync and clear failure diagnostics.
