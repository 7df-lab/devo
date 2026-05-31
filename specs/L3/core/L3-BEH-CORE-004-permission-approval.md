---
artifact_id: L3-BEH-CORE-004
revision: 2
status: Draft
active_baseline: no
---

# L3-BEH-CORE-004 — Permission Evaluation and Approval Pipeline

## Purpose

Define the permission profile resolution, access evaluation functions, the `authorize_tool_request()` entry point with four-layer pipeline, approval scoping/caching, auto-reviewer circuit breaker, and escalation tiers.

## Source Design

L2-DES-SAFETY-001, L2-DES-SAFETY-002, L3-DES-ARCH-001

## 1. Permission Types (in core)

```rust
pub struct PermissionProfile {
    pub filesystem_policy: Vec<FsPolicyEntry>,
    pub network_policy: NetworkPolicy,
}

pub struct FsPolicyEntry {
    pub path: PathBuf,           // materialized absolute path
    pub access: AccessMode,      // Read | Write | None
    pub is_explicit: bool,       // false for auto-generated protected-metadata entries
}

pub enum AccessMode { Read, Write, None }

pub struct NetworkPolicy {
    pub enabled: bool,
    pub allowed_domains: Vec<DomainPattern>,
    pub denied_domains: Vec<DomainPattern>,
    pub proxy_url: Option<url::Url>,
}

pub struct RuntimePermissionProfile {
    pub profile: PermissionProfile,
    pub workspace_roots: Vec<PathBuf>,
    pub additional_per_call: Option<AdditionalPermissions>,
}
```

## 2. Profile Resolution

```rust
/// Resolve a named profile (":read-only", ":workspace", ":danger-full-access", or custom name)
/// into a concrete PermissionProfile with all symbolic paths materialized.
pub fn resolve_permission_profile(
    name: &str,
    workspace_roots: &[PathBuf],
    custom_profiles: &HashMap<String, CustomProfile>,
) -> Result<PermissionProfile, ProfileError>;

pub fn materialize_workspace_roots(
    profile: &mut PermissionProfile,
    roots: &[PathBuf],
);
```

### Built-in Profile Definitions

**`:read-only`**: root = `Read`, no `Write` entries, network = disabled.

**`:workspace`**: root = `Read`, workspace_roots = `Write`, tmpdir = `Write`, network = disabled. Auto-add read-only carveouts for `.git`, `.devo`, `.agents` within writable roots.

**`:danger-full-access`**: root = `Write`, network = enabled. No carveouts.

### Access Evaluation

```rust
pub fn resolve_access(path: &Path, profile: &PermissionProfile) -> AccessMode;

pub fn can_read(path: &Path, profile: &PermissionProfile) -> bool {
    matches!(resolve_access(path, profile), AccessMode::Read | AccessMode::Write)
}

pub fn can_write(path: &Path, profile: &PermissionProfile) -> bool {
    matches!(resolve_access(path, profile), AccessMode::Write)
}

pub fn network_enabled(profile: &PermissionProfile, host: &str) -> bool;
```

**Precedence**: Most specific path (longest prefix match) wins. At equal specificity: `None` > `Write` > `Read`.

## 3. authorize_tool_request() — Central Entry Point

```rust
pub struct ToolPermissionRequest {
    pub tool_name: String,
    pub tool_category: ToolCategory,
    pub resource: ResourceKind,
    pub path: Option<PathBuf>,
    pub host: Option<String>,
    pub command: Option<String>,
    pub command_description: Option<String>,
    pub justification: Option<String>,
    pub sandbox_mode: SandboxMode,
}

pub enum ResourceKind {
    FileRead, FileWrite, ShellExec, Network, ExternalTool,
}

pub enum SandboxMode {
    Normal,
    AdditionalPermissions(AdditionalPermissions),
    RequireEscalated { justification: String, prefix_rule: Option<Vec<String>> },
}

pub enum PermissionDecision {
    Allow,
    Deny { reason: String },
    Ask { approval_id: ApprovalId, summary: String, details: String, available_scopes: Vec<ApprovalScope>, expires_at: Option<DateTime<Utc>> },
}

pub fn authorize_tool_request(
    request: &ToolPermissionRequest,
    profile: &RuntimePermissionProfile,
    cache: &mut ApprovalCache,
    policy: &ApprovalPolicy,
    reviewer: Option<&dyn AutoReviewer>,
    hooks: &[Box<dyn ApprovalHook>],
) -> PermissionDecision;
```

## 4. Four-Layer Pipeline

```
Layer 1 — PermissionMode override
  If session PermissionMode == AutoApprove → Allow
  If session PermissionMode == Deny → Deny
  Else → continue

Layer 2 — Profile evaluation
  If action is within PermissionProfile → Allow
  If outside → continue

Layer 3 — Hooks → Cache → Auto-Reviewer
  a) Run hooks: any Deny → Deny. All Allow → Allow. No decision → continue.
  b) Check cache: (tool_name, resource, scope) match cached decision → cached Allow/Deny.
  c) Run auto-reviewer (if configured):
     - Build compact prompt (≤500 tokens) with tool, resource, justification, safety rules
     - Call fast model. Parse JSON { decision: "approve"|"deny"|"uncertain" }.
     - approve → Allow. deny → Deny. uncertain / timeout / parse error → continue.
     - Update circuit breaker counters.

Layer 4 — User prompt
  Create PendingApproval, emit approval/requested to clients, return Ask.
```

## 5. Approval Cache

```rust
pub struct ApprovalCache {
    turn_tools: HashSet<(String, ResourceKind)>,
    session_tools: HashSet<(String, ResourceKind)>,
    turn_hosts: HashSet<String>,
    session_hosts: HashSet<String>,
    turn_path_prefixes: HashSet<PathBuf>,
    session_path_prefixes: HashSet<PathBuf>,
    turn_command_prefixes: HashSet<Vec<String>>,
    session_command_prefixes: HashSet<Vec<String>>,
    denied_session: HashSet<(String, ResourceKind)>,
}

pub enum ApprovalScope {
    Once,
    Turn,
    Session,
    PathPrefix(PathBuf),
    Host(String),
    CommandPrefix(Vec<String>),
    McpTool { server: String, tool: String },
}
```

**Cache lifecycle**: Turn-level cleared after each turn. Session-level persists for session lifetime.

## 6. Auto-Reviewer Circuit Breaker

```rust
pub struct AutoReviewerState {
    consecutive_denials: u32,
    denials_in_window: VecDeque<DateTime<Utc>>,
    tripped: bool,
}

impl AutoReviewerState {
    pub fn check_and_update(&mut self, decision: &ReviewDecision) -> AutoReviewerStatus {
        if self.tripped { return AutoReviewerStatus::Tripped; }

        match decision {
            ReviewDecision::Deny => {
                self.consecutive_denials += 1;
                self.denials_in_window.push_back(Utc::now());
                // Prune window: keep last 50 requests
                while self.denials_in_window.len() > 50 {
                    self.denials_in_window.pop_front();
                }
            }
            ReviewDecision::Approve => {
                self.consecutive_denials = 0;
                self.denials_in_window.push_back(Utc::now());
            }
            ReviewDecision::Uncertain => { /* no change to counters */ }
        }

        if self.consecutive_denials >= 3 || self.denials_in_window.len() >= 10 {
            self.tripped = true;
            AutoReviewerStatus::Tripped
        } else {
            AutoReviewerStatus::Active
        }
    }
}
```

## 7. Escalation Tiers

**Tier 1 — Additional Permissions**: `sandbox_mode = AdditionalPermissions(block)`. Merged into `RuntimePermissionProfile` for ONE invocation. Additional paths that overlap base `None` entries → `None` wins (cannot override explicit denies).

**Tier 2 — Full Escalation**: `sandbox_mode = RequireEscalated { justification, prefix_rule }`. Command runs outside sandbox. ALWAYS requires user approval unless a prefix rule matches. If user approves and prefix_rule provided → add to `session_command_prefixes` in cache.

**Prefix rule restrictions**: Cannot match `rm`, `dd`, `mkfs`, `:(){`, heredocs. Cannot contain credential patterns. Matched exact token-by-token against first command segment only (split on `|`, `&&`, `||`, `;`).

## 8. Async Behavior

| Operation | Timeout | Retries | Cancel |
|---|---|---|---|
| `authorize_tool_request()` | Synchronous (no await needed for Layers 1-3) | N/A | N/A |
| Auto-reviewer model call | 10s | 0 | Timeout → `uncertain` (fall through to user) |
| User approval wait | `approval_timeout` (default 300s) | 0 | Timeout → auto-deny |
| Approval hook script | 5s per hook | 0 | Timeout → no decision (continue) |

## 9. State Machine — Approval Lifecycle

```
RequestReceived ──► Evaluating (Layers 1-3)
Evaluating ──► Allowed      (Layer 1-3 decided Allow)
Evaluating ──► Denied       (Layer 1-3 decided Deny)
Evaluating ──► PendingUser  (Layer 4 — Ask returned)
PendingUser ──► Allowed     (user approved)
PendingUser ──► Denied      (user denied)
PendingUser ──► Expired     (timeout)
```

**Illegal transitions**:
- `Allowed/Denied → PendingUser` (decision is final)
- `Expired → Allowed` (late user response must be rejected with "already resolved")

## Traceability

| L2 Source | Relationship |
|---|---|
| L2-DES-SAFETY-001 | specified-by |
| L2-DES-SAFETY-002 | specified-by |
| L3-DES-ARCH-001 | specified-by |

## Implementation Placement Guidance

- `authorize_tool_request()` is a pure synchronous function for Layers 1-3. Layer 4 (user prompt) returns `Ask` and the server handles the async wait.
- Permission-domain types such as `PermissionProfile`, `AccessMode`, and `RuntimePermissionProfile` belong to core because they decide tool authorization.
- The safety crate owns OS/process-boundary enforcement primitives such as `Sandbox`, `SandboxPolicy`, process constraining, and network egress filtering.
- Auto-reviewer uses the session's provider with a minimal, structured prompt. Result parsing is strict JSON.
