use super::*;
use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::json_schema::JsonSchema;
use crate::registry::ToolRegistryBuilder;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolPreparationFeedback;
use crate::tool_spec::ToolSpec;
use async_trait::async_trait;
use pretty_assertions::assert_eq;

fn empty_schema() -> JsonSchema {
    JsonSchema::object(Default::default(), None, None)
}

fn spec(
    name: &str,
    mode: ToolExecutionMode,
    tags: Vec<ToolCapabilityTag>,
    parallel: bool,
    output: ToolOutputMode,
) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: String::new(),
        input_schema: empty_schema(),
        output_mode: output,
        execution_mode: mode,
        capability_tags: tags,
        supports_parallel: parallel,
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    }
}

fn call(id: &str, name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        input,
    }
}

fn empty_call(id: &str, name: &str) -> ToolCall {
    call(id, name, serde_json::json!({}))
}

fn register(
    builder: &mut ToolRegistryBuilder,
    name: &str,
    handler: Arc<dyn ToolHandler>,
    spec: ToolSpec,
) {
    builder.register_handler(name, handler);
    builder.push_spec(spec);
}

fn no_perm_runtime(registry: Arc<ToolRegistry>) -> ToolRuntime {
    ToolRuntime::new_without_permissions(registry)
}

struct FixedTool {
    spec: ToolSpec,
    output: &'static str,
}

impl FixedTool {
    fn read() -> Self {
        Self {
            spec: ToolSpec::new("read_tool", "read", empty_schema()),
            output: "read ok",
        }
    }
    fn write() -> Self {
        Self {
            spec: ToolSpec::new("write_tool", "write", empty_schema()),
            output: "write ok",
        }
    }
}

#[async_trait]
impl ToolHandler for FixedTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    async fn handle(
        &self,
        _ctx: ToolContext,
        _input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        Ok(ToolResult::success(
            ToolResultContent::Text(self.output.into()),
            self.output,
        ))
    }
}

struct DelayedReadTool {
    spec: ToolSpec,
}

impl DelayedReadTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec::new("delayed_read_tool", "delayed read", empty_schema()),
        }
    }
}

#[async_trait]
impl ToolHandler for DelayedReadTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    async fn handle(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let delay_ms = input
            .get("delay_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        let output = input
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        Ok(ToolResult::success(
            ToolResultContent::Text(output.to_string()),
            "done",
        ))
    }
}

fn make_registry() -> Arc<ToolRegistry> {
    let mut b = ToolRegistryBuilder::new();
    register(
        &mut b,
        "read_tool",
        Arc::new(FixedTool::read()),
        spec(
            "read_tool",
            ToolExecutionMode::ReadOnly,
            vec![],
            true,
            ToolOutputMode::Text,
        ),
    );
    register(
        &mut b,
        "read",
        Arc::new(FixedTool::read()),
        spec(
            "read",
            ToolExecutionMode::ReadOnly,
            vec![ToolCapabilityTag::ReadFiles],
            true,
            ToolOutputMode::Text,
        ),
    );
    register(
        &mut b,
        "write_tool",
        Arc::new(FixedTool::write()),
        spec(
            "write_tool",
            ToolExecutionMode::Mutating,
            vec![ToolCapabilityTag::WriteFiles],
            false,
            ToolOutputMode::Text,
        ),
    );
    register(
        &mut b,
        "delayed_read_tool",
        Arc::new(DelayedReadTool::new()),
        spec(
            "delayed_read_tool",
            ToolExecutionMode::ReadOnly,
            vec![],
            true,
            ToolOutputMode::Text,
        ),
    );
    Arc::new(b.build())
}

fn capture_permission(
    grant_ok: bool,
) -> (
    PermissionChecker,
    tokio::sync::oneshot::Receiver<ToolPermissionRequest>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = std::sync::Mutex::new(Some(tx));
    let checker = PermissionChecker::new(move |request| {
        tx.lock()
            .expect("lock sender")
            .take()
            .expect("send once")
            .send(request)
            .expect("receiver still alive");
        Box::pin(async move {
            if grant_ok {
                Ok(PermissionGrant::default())
            } else {
                Err("denied".into())
            }
        })
    });
    (checker, rx)
}

#[tokio::test]
async fn unknown_tool_returns_error() {
    let result = no_perm_runtime(make_registry())
        .execute_single(&empty_call("c1", "nonexistent"), &None)
        .await;
    assert!(result.is_error);
    assert!(result.content.into_string().contains("unknown tool"));
}

#[tokio::test]
async fn subagent_runtime_blocks_parent_agent_coordination_tools() {
    let runtime = ToolRuntime::new_with_context(
        make_registry(),
        PermissionChecker::always_allow(),
        ToolRuntimeContext {
            agent_scope: ToolAgentScope::Subagent,
            ..ToolRuntimeContext::default()
        },
    );
    for name in [
        "spawn_agent",
        "spawn-agent",
        "spawnagent",
        "spawn_subagent",
        "spawn-subagent",
        "subagent",
        "sub_agent",
        "delegate",
        "send_message",
        "send-message",
        "sendmessage",
        "await_task",
        "await-task",
        "awaittask",
        "list_tasks",
        "list-tasks",
        "listtasks",
        "cancel_task",
        "cancel-task",
        "canceltask",
        "wait_agent",
        "wait-agent",
        "waitagent",
        "subagent_result",
        "subagent-result",
        "list_agents",
        "list-agents",
        "listagents",
        "subagent_status",
        "subagent-status",
        "close_agent",
        "close-agent",
        "closeagent",
    ] {
        let result = runtime
            .execute_single(&empty_call(&format!("call-{name}"), name), &None)
            .await;
        assert!(result.is_error);
        assert_eq!(
            result.content.into_string(),
            "sub-agents cannot use parent-agent coordination tools"
        );
    }
}

#[tokio::test]
async fn read_only_tool_succeeds() {
    let result = no_perm_runtime(make_registry())
        .execute_single(&empty_call("c1", "read_tool"), &None)
        .await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn execute_batch_runs_all_tools() {
    let results = no_perm_runtime(make_registry())
        .execute_batch(&[
            empty_call("c1", "read_tool"),
            empty_call("c2", "write_tool"),
        ])
        .await;
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| !r.is_error));
}

#[tokio::test]
async fn permission_checker_allow() {
    assert!(
        PermissionChecker::always_allow()
            .check(test_permission_request("any_tool"))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn permission_checker_deny() {
    let checker = PermissionChecker::new(|request| {
        let n = request.tool_name;
        Box::pin(async move {
            if n == "blocked" {
                Err("blocked".into())
            } else {
                Ok(PermissionGrant::default())
            }
        })
    });
    assert!(
        checker
            .check(test_permission_request("allowed"))
            .await
            .is_ok()
    );
    assert!(
        checker
            .check(test_permission_request("blocked"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn runtime_denies_mutating_with_deny_checker() {
    let runtime = ToolRuntime::new(
        make_registry(),
        PermissionChecker::new(|request| {
            let n = request.tool_name;
            Box::pin(async move { Err(format!("{n} denied")) })
        }),
    );
    let read_result = runtime
        .execute_single(&empty_call("c1", "read_tool"), &None)
        .await;
    assert!(
        !read_result.is_error,
        "read-only tool should bypass permission check"
    );
    let write_result = runtime
        .execute_single(&empty_call("c2", "write_tool"), &None)
        .await;
    assert!(write_result.is_error, "mutating tool should be denied");
    assert!(
        write_result
            .content
            .into_string()
            .contains("permission denied")
    );
}

#[tokio::test]
async fn runtime_checks_file_read_tools() {
    let (checker, rx) = capture_permission(false);
    let runtime = ToolRuntime::new_with_context(
        make_registry(),
        checker,
        ToolRuntimeContext {
            cwd: PathBuf::from("C:/workspace"),
            ..ToolRuntimeContext::default()
        },
    );
    let result = runtime
        .execute_single(
            &call(
                "call-read",
                "read",
                serde_json::json!({ "filePath": "src/lib.rs" }),
            ),
            &None,
        )
        .await;
    let request = rx.await.expect("permission request");
    assert!(result.is_error);
    assert_eq!(request.tool_name, "read");
    assert_eq!(request.resource, devo_safety::ResourceKind::FileRead);
    assert_eq!(
        request.path,
        Some(PathBuf::from("C:/workspace").join("src/lib.rs"))
    );
    assert!(result.content.into_string().contains("permission denied"));
}

#[tokio::test]
async fn mutating_tool_permission_request_carries_context_and_summary() {
    let (checker, rx) = capture_permission(true);
    let runtime = ToolRuntime::new_with_context(
        make_registry(),
        checker,
        ToolRuntimeContext {
            session_id: "session-1".into(),
            turn_id: Some("turn-1".into()),
            cwd: PathBuf::from("C:/workspace"),
            agent_scope: ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            ..ToolRuntimeContext::default()
        },
    );
    let result = runtime
        .execute_single(
            &call(
                "call-1",
                "write_tool",
                serde_json::json!({ "filePath": "src/main.rs" }),
            ),
            &None,
        )
        .await;
    let request = rx.await.expect("permission request");
    assert!(!result.is_error);
    assert_eq!(
        (
            request.tool_call_id.as_str(),
            request.tool_name.as_str(),
            request.session_id.as_str(),
            request.turn_id.map(|id| id.as_str()),
            request.resource
        ),
        (
            "call-1",
            "write_tool",
            "session-1",
            Some("turn-1"),
            devo_safety::ResourceKind::FileWrite
        )
    );
}

#[tokio::test]
async fn bash_alias_uses_shell_command_permission_metadata() {
    let mut b = ToolRegistryBuilder::new();
    let handler: Arc<dyn ToolHandler> = Arc::new(FixedTool::write());
    b.register_handler("shell_command", Arc::clone(&handler));
    register(
        &mut b,
        "bash",
        handler,
        spec(
            "shell_command",
            ToolExecutionMode::Mutating,
            vec![ToolCapabilityTag::ExecuteProcess],
            false,
            ToolOutputMode::Text,
        ),
    );
    let (checker, rx) = capture_permission(false);
    let result = ToolRuntime::new(Arc::new(b.build()), checker)
        .execute_single(
            &call(
                "call-1",
                "bash",
                serde_json::json!({ "command": "git status" }),
            ),
            &None,
        )
        .await;
    let request = rx.await.expect("permission request");
    assert!(result.is_error);
    assert_eq!(request.tool_name, "shell_command");
    assert_eq!(request.resource, devo_safety::ResourceKind::ShellExec);
    assert_eq!(request.target.as_deref(), Some("git status"));
    assert_eq!(
        request.command_prefix,
        Some(vec!["git".to_string(), "status".to_string()])
    );
}

#[test]
fn path_for_tool_input_cases() {
    let cwd = Path::new("C:/workspace");
    let cases: &[(&str, serde_json::Value, Option<PathBuf>)] = &[
        (
            "write",
            serde_json::json!({ "filePath": "src/lib.rs" }),
            Some(PathBuf::from("C:/workspace").join("src/lib.rs")),
        ),
        (
            "grep",
            serde_json::json!({ "pattern": "needle" }),
            Some(PathBuf::from("C:/workspace").join(".")),
        ),
    ];
    for (tool, input, expected) in cases {
        assert_eq!(path_for_tool_input(tool, input, cwd), *expected, "{tool}");
    }
}

#[test]
fn host_from_url_ignores_scheme_and_path() {
    assert_eq!(
        host_from_url("https://example.com/docs/index.html"),
        Some("example.com".into())
    );
}

#[test]
fn command_prefix_uses_first_command_tokens() {
    assert_eq!(
        command_prefix("git add -A"),
        Some(vec!["git".to_string(), "add".to_string()])
    );
    assert_eq!(
        command_prefix("'cargo' test --all"),
        Some(vec!["cargo".to_string(), "test".to_string()])
    );
}

#[test]
fn command_prefix_rejects_complex_shell_features() {
    for cmd in [
        "git add -A | tee out.txt",
        "npm test > output.txt",
        "echo $(pwd)",
        "echo $HOME",
        "FOO=bar cargo test",
        "(pwd)",
        "rg *.rs",
        "cargo fmt && cargo test",
    ] {
        assert_eq!(command_prefix(cmd), None, "{cmd}");
    }
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: prefix_rule on exec_command, shell_command, and bash overrides the derived command prefix.
#[test]
fn shell_family_prefix_rule_overrides_derived_prefix() {
    for tool_name in ["exec_command", "shell_command", "bash"] {
        assert_eq!(
            command_prefix_for_tool_input(
                tool_name,
                &serde_json::json!({
                    "cmd": "git add -A",
                    "command": "git add -A",
                    "prefix_rule": ["cargo", "test"]
                })
            ),
            Some(vec!["cargo".to_string(), "test".to_string()]),
            "{tool_name}"
        );
    }
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: a banned prefix_rule is not offered for exec_command, shell_command, or bash.
#[test]
fn shell_family_banned_prefix_rule_is_not_offered() {
    for tool_name in ["exec_command", "shell_command", "bash"] {
        assert_eq!(
            command_prefix_for_tool_input(
                tool_name,
                &serde_json::json!({
                    "cmd": "git status",
                    "command": "git status",
                    "prefix_rule": ["git"]
                })
            ),
            None,
            "{tool_name}"
        );
    }
}

fn strs(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| token.to_string()).collect()
}

#[test]
fn command_pattern_generalize_and_reject_cases() {
    let cwd = std::path::Path::new("/nonexistent-cwd-for-pattern-tests");
    let some = |tokens: &[&str]| Some(strs(tokens));
    let cases: &[(&str, Option<Vec<String>>)] = &[
        ("git add file.txt", some(&["git", "add", "*"])),
        ("node -e 'foo.bar'", None),
        (
            "git commit -m 'initial commit'",
            some(&["git", "commit", "-m", "*"]),
        ),
        ("git add a.txt b.txt", some(&["git", "add", "*", "*"])),
        ("git add file.txt docs", some(&["git", "add", "*", "*"])),
        (
            "cargo build --release",
            some(&["cargo", "build", "--release"]),
        ),
        ("git status", some(&["git", "status"])),
        ("sudo rm -rf /tmp/x", None),
        ("rm /tmp/x", None),
        ("dd if=/dev/zero of=x", None),
        ("/usr/bin/sudo ls", None),
        ("sh -c 'ls'", None),
        ("bash -c 'ls'", None),
        ("find . -name foo", None),
        ("xargs rm", None),
        ("env ls", None),
        ("eval ls", None),
        ("git add a && git commit", None),
        ("cat x | grep y", None),
        ("echo hi > out.txt", None),
        ("echo $(pwd)", None),
        ("FOO=bar cargo test", None),
        ("sleep 1 & touch /tmp/x", None),
        ("git add 'unterminated", None),
        ("git a b c d e f g h i j k l m n o p", None),
        ("git add A B C D E F G H I", None),
        (
            "git add A B C D E F G H",
            some(&["git", "add", "*", "*", "*", "*", "*", "*", "*", "*"]),
        ),
    ];
    for (cmd, expected) in cases {
        assert_eq!(generalize_command_pattern(cmd, cwd), *expected, "{cmd}");
    }
    assert!(!command_contains_standalone_ampersand(
        r#"curl "http://example.com/?a=1&b=2""#
    ));
    assert!(command_contains_standalone_ampersand("sleep 1 & touch x"));
    assert!(command_contains_standalone_ampersand("sleep 1& rm x"));
    assert!(!command_contains_standalone_ampersand("true && false"));
    assert!(!token_is_background_ampersand("&&"));
    assert!(token_is_background_ampersand("1&"));
    assert!(token_is_background_ampersand("&"));
}

#[test]
fn command_pattern_keeps_subcommand_words_without_cwd_stat() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let cwd = tempdir.path();
    std::fs::write(cwd.join("add"), "placeholder").expect("write placeholder");
    assert_eq!(
        generalize_command_pattern("git add", cwd),
        Some(strs(&["git", "add"]))
    );
}

#[test]
fn command_pattern_match_cases() {
    let cases: &[(&[&str], &[&str], bool)] = &[
        (&["git", "add", "*"], &["git", "add", "file.txt"], true),
        (&["git", "add", "*"], &["git", "add", "a", "b"], true),
        (&["git", "add", "*"], &["git", "add"], false),
        (&["git", "add", "*"], &["git", "commit", "x"], false),
        (&["git", "add", "*"], &["sudo", "git", "add", "x"], false),
        (
            &["git", "commit", "-m", "*", "--amend"],
            &["git", "commit", "-m", "msg", "--amend"],
            true,
        ),
        (
            &["git", "commit", "-m", "*", "--amend"],
            &["git", "commit", "-m", "a", "b", "--amend"],
            false,
        ),
        (
            &["git", "commit", "-m", "*", "--amend"],
            &["git", "commit", "-m", "--amend"],
            false,
        ),
        (&["git", "status"], &["git", "status"], true),
        (&["git", "status"], &["git", "status", "-s"], false),
        (&["git", "status"], &["git"], false),
    ];
    for (pattern, argv, expected) in cases {
        assert_eq!(
            command_pattern_matches(&strs(pattern), &strs(argv)),
            *expected,
            "{pattern:?} vs {argv:?}"
        );
    }
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: sandbox permission inputs are classified into explicit tiers.
#[test]
fn explicit_sandbox_permissions_are_classified() {
    let input_path = std::env::temp_dir().join("input");
    let output_path = std::env::temp_dir().join("output");
    let legacy_path = std::env::temp_dir().join("legacy");
    let input_path_s = input_path.to_string_lossy().to_string();
    let output_path_s = output_path.to_string_lossy().to_string();
    let legacy_path_s = legacy_path.to_string_lossy().to_string();

    assert_eq!(
        sandbox_permission_request_from_input(&serde_json::json!({
        "sandbox_permissions": "require_escalated"
        }))
        .expect("full escalation request"),
        SandboxPermissionRequest::FullEscalation
    );
    assert_eq!(
        sandbox_permission_request_from_input(&serde_json::json!({
            "sandbox_permissions": "with_additional_permissions",
            "additional_permissions": {
                "network": {"enabled": true},
                "file_system": {
                    "read": [&input_path_s],
                    "write": [&output_path_s]
                }
            }
        }))
        .expect("additional permissions request"),
        SandboxPermissionRequest::AdditionalPermissions(AdditionalSandboxPermissions {
            network: NetworkPermission::Enabled,
            read_paths: vec![input_path],
            write_paths: vec![output_path],
        })
    );
    assert_eq!(
        sandbox_permission_request_from_input(&serde_json::json!({
            "additional_permissions": {
                "file_system": {"read": [&legacy_path_s]}
            }
        }))
        .expect("legacy additional permissions request"),
        SandboxPermissionRequest::AdditionalPermissions(AdditionalSandboxPermissions {
            network: NetworkPermission::Unchanged,
            read_paths: vec![legacy_path],
            write_paths: vec![],
        })
    );
    assert_eq!(
        sandbox_permission_request_from_input(&serde_json::json!({
        "sandbox_permissions": "use_default"
        }))
        .expect("default sandbox request"),
        SandboxPermissionRequest::Default
    );
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: an all-unchanged additional_permissions object (codex-style
/// clients always echo the full object) is semantically Default, not an
/// error — with `use_default` mode, no mode at all, or an empty object.
#[test]
fn all_unchanged_additional_permissions_are_default() {
    for input in [
        serde_json::json!({
            "sandbox_permissions": "use_default",
            "additional_permissions": {
                "network": {"enabled": false},
                "file_system": {"read": [], "write": []}
            }
        }),
        serde_json::json!({
            "additional_permissions": {
                "network": {"enabled": false},
                "file_system": {"read": [], "write": []}
            }
        }),
        serde_json::json!({
            "additional_permissions": {}
        }),
    ] {
        assert_eq!(
            sandbox_permission_request_from_input(&input)
                .expect("all-unchanged request should be Default"),
            SandboxPermissionRequest::Default,
            "input should classify as Default: {input}"
        );
    }
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: malformed or ambiguous sandbox requests fail closed.
#[test]
fn sandbox_permission_request_rejects_invalid_inputs() {
    for input in [
        serde_json::json!({
            "additional_permissions": {
                "file_system": {"read": ["relative/path"]}
            }
        }),
        serde_json::json!({
            "sandbox_permissions": "require_escalated",
            "additional_permissions": {
                "file_system": {"read": ["/tmp/input"]}
            }
        }),
    ] {
        assert!(
            sandbox_permission_request_from_input(&input).is_err(),
            "input should be rejected: {input}"
        );
    }
}

/// Trace: L2-DES-SAFETY-002
/// Verifies: permission-cache keys use normalized tiers and path sets.
#[test]
fn sandbox_permission_cache_key_normalizes_additional_permissions() {
    let a_path = std::env::temp_dir().join("a");
    let b_path = std::env::temp_dir().join("b");
    let a_path_s = a_path.to_string_lossy().to_string();
    let b_path_s = b_path.to_string_lossy().to_string();
    let first = sandbox_permission_cache_key_from_input(&serde_json::json!({
        "sandbox_permissions": "with_additional_permissions",
        "additional_permissions": {
            "file_system": {"read": [&b_path_s, &a_path_s]}
        }
    }));
    let second = sandbox_permission_cache_key_from_input(&serde_json::json!({
        "additional_permissions": {
            "file_system": {"read": [&a_path_s, &b_path_s]}
        }
    }));
    assert_eq!(first, second);
    assert_ne!(
        first,
        sandbox_permission_cache_key_from_input(&serde_json::json!({
            "sandbox_permissions": "require_escalated"
        }))
    );
}

#[test]
fn sandbox_profile_inactive_detection() {
    for (profile, inactive) in [
        (None, true),
        (Some(""), true),
        (Some("off"), true),
        (Some("none"), true),
        (Some("workspace"), false),
    ] {
        assert_eq!(
            sandbox_profile_is_inactive(profile),
            inactive,
            "{profile:?}"
        );
    }
}

#[test]
fn tool_result_detects_sandbox_denied_prefix() {
    let denied = ToolResult::error(
        ToolResultContent::Text("SANDBOX_DENIED: blocked".into()),
        "failed",
        ToolCallError::ExecutionFailed("SANDBOX_DENIED: blocked".into()),
    );
    assert!(tool_result_is_sandbox_denied(&denied));
    let other = ToolResult::error(
        ToolResultContent::Text("exit code 1".into()),
        "failed",
        ToolCallError::ExecutionFailed("exit code 1".into()),
    );
    assert!(!tool_result_is_sandbox_denied(&other));
}

struct SandboxAwareTool {
    spec: ToolSpec,
    attempts: Arc<std::sync::Mutex<Vec<Option<String>>>>,
    succeed_outside: bool,
}

#[async_trait]
impl ToolHandler for SandboxAwareTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    async fn handle(
        &self,
        ctx: ToolContext,
        _input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        self.attempts
            .lock()
            .expect("attempts lock")
            .push(ctx.sandbox_profile.clone());
        if self.succeed_outside && ctx.sandbox_profile.as_deref() != Some("workspace") {
            return Ok(ToolResult::success(
                ToolResultContent::Text("ok outside sandbox".into()),
                "ok",
            ));
        }
        Ok(ToolResult::error(
            ToolResultContent::Text(
                "SANDBOX_DENIED: The command was blocked by the OS sandbox.".into(),
            ),
            "denied",
            ToolCallError::ExecutionFailed("SANDBOX_DENIED".into()),
        ))
    }
}

fn shell_spec() -> ToolSpec {
    spec(
        "shell_command",
        ToolExecutionMode::Mutating,
        vec![ToolCapabilityTag::ExecuteProcess],
        false,
        ToolOutputMode::Text,
    )
}

fn sandbox_runtime(
    succeed_outside: bool,
    from_approval: bool,
    profile: &str,
    cwd: Option<PathBuf>,
) -> (ToolRuntime, Arc<std::sync::Mutex<Vec<Option<String>>>>) {
    let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut b = ToolRegistryBuilder::new();
    register(
        &mut b,
        "shell_command",
        Arc::new(SandboxAwareTool {
            spec: shell_spec(),
            attempts: Arc::clone(&attempts),
            succeed_outside,
        }),
        shell_spec(),
    );
    let checker = PermissionChecker::new(move |_| {
        Box::pin(async move {
            if from_approval {
                Ok(PermissionGrant::from_approval(
                    &SandboxPermissionRequest::Default,
                ))
            } else {
                Ok(PermissionGrant::default())
            }
        })
    });
    let mut runtime = ToolRuntime::new(Arc::new(b.build()), checker);
    if let Some(cwd) = cwd {
        runtime.context.cwd = cwd;
    }
    runtime.context.sandbox_profile = Some(profile.to_string());
    (runtime, attempts)
}

#[tokio::test]
async fn execute_single_retries_without_sandbox_when_already_approved() {
    let permission_checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let permission_checks_for_checker = Arc::clone(&permission_checks);
    let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut b = ToolRegistryBuilder::new();
    register(
        &mut b,
        "shell_command",
        Arc::new(SandboxAwareTool {
            spec: shell_spec(),
            attempts: Arc::clone(&attempts),
            succeed_outside: true,
        }),
        shell_spec(),
    );
    let checker = PermissionChecker::new(move |_request| {
        permission_checks_for_checker.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async {
            Ok(PermissionGrant::from_approval(
                &SandboxPermissionRequest::Default,
            ))
        })
    });
    let mut runtime = ToolRuntime::new(Arc::new(b.build()), checker);
    runtime.context.sandbox_profile = Some("workspace".to_string());
    let result = runtime
        .execute_single(
            &call(
                "deny1",
                "shell_command",
                serde_json::json!({ "command": "touch /tmp/x" }),
            ),
            &None,
        )
        .await;
    assert!(!result.is_error);
    assert_eq!(result.content.into_string(), "ok outside sandbox");
    assert_eq!(
        permission_checks.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "SANDBOX_DENIED retry must not request a second permission check"
    );
    assert_eq!(
        *attempts.lock().expect("attempts lock"),
        vec![Some("workspace".to_string()), Some("off".to_string())]
    );
}

#[tokio::test]
async fn execute_single_does_not_silent_unsandbox_without_approval() {
    let (runtime, attempts) = sandbox_runtime(false, false, "workspace", None);
    let result = runtime
        .execute_single(
            &call(
                "deny2",
                "shell_command",
                serde_json::json!({ "command": "touch /tmp/x" }),
            ),
            &None,
        )
        .await;
    assert!(result.is_error);
    assert!(
        result.content.into_string().starts_with("SANDBOX_DENIED:"),
        "denial must surface for require_escalated"
    );
    assert_eq!(
        *attempts.lock().expect("attempts lock"),
        vec![Some("workspace".to_string())],
        "must not silent unsandbox without already_approved"
    );
}

#[tokio::test]
async fn execute_single_skips_unsandbox_retry_when_profile_has_deny_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path();
    let config_dir = workspace.join(".devo");
    std::fs::create_dir_all(&config_dir).expect("mkdir");
    std::fs::write(
        config_dir.join("sandbox.toml"),
        r#"
[profiles.locked]
extends = "workspace"
deny = ["/etc/passwd"]
"#,
    )
    .expect("write config");
    let (runtime, attempts) = sandbox_runtime(false, true, "locked", Some(workspace.to_path_buf()));
    let result = runtime
        .execute_single(
            &call(
                "deny3",
                "shell_command",
                serde_json::json!({ "command": "cat /etc/passwd" }),
            ),
            &None,
        )
        .await;
    assert!(result.is_error);
    assert_eq!(
        *attempts.lock().expect("attempts lock"),
        vec![Some("locked".to_string())],
        "deny-read profiles must not silent-unsandbox after SANDBOX_DENIED"
    );
}

fn test_permission_request(tool_name: &str) -> ToolPermissionRequest {
    ToolPermissionRequest {
        tool_call_id: "call".into(),
        tool_name: tool_name.into(),
        input: serde_json::json!({}),
        cwd: std::path::PathBuf::new(),
        session_id: "session".into(),
        turn_id: Some("turn".into()),
        resource: devo_safety::ResourceKind::Custom(tool_name.into()),
        action_summary: tool_name.into(),
        justification: None,
        path: None,
        host: None,
        target: None,
        command_prefix: None,
        command_argv: None,
        command_pattern: None,
        sandbox_permissions: SandboxPermissionRequest::Default,
    }
}

#[tokio::test]
async fn runtime_concurrent_then_sequential() {
    let results = no_perm_runtime(make_registry())
        .execute_batch(&[
            empty_call("r1", "read_tool"),
            empty_call("r2", "read_tool"),
            empty_call("w1", "write_tool"),
        ])
        .await;
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| !r.is_error));
    assert_eq!(results[0].tool_use_id, "r1".to_string());
    assert_eq!(results[1].tool_use_id, "r2".to_string());
}

#[tokio::test]
async fn parallel_completion_callback_streams_before_batch_is_done_but_results_stay_ordered() {
    let runtime = no_perm_runtime(make_registry());
    let calls = vec![
        call(
            "slow",
            "delayed_read_tool",
            serde_json::json!({ "delay_ms": 50, "output": "slow output" }),
        ),
        call(
            "fast",
            "delayed_read_tool",
            serde_json::json!({ "delay_ms": 5, "output": "fast output" }),
        ),
    ];
    let completions = Arc::new(std::sync::Mutex::new(Vec::new()));
    let completions_clone = Arc::clone(&completions);
    let results = runtime
        .execute_batch_streaming_with_completion(
            &calls,
            |_tool_use_id, _content| Box::pin(async {}),
            move |result| {
                let completions_clone = Arc::clone(&completions_clone);
                Box::pin(async move {
                    completions_clone
                        .lock()
                        .expect("lock completions")
                        .push(result.tool_use_id.clone());
                })
            },
        )
        .await;
    assert_eq!(
        completions.lock().expect("lock completions").as_slice(),
        &["fast".to_string(), "slow".to_string()]
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.tool_use_id.as_str())
            .collect::<Vec<_>>(),
        vec!["slow", "fast"]
    );
}

#[tokio::test]
async fn runtime_empty_batch() {
    assert!(
        no_perm_runtime(make_registry())
            .execute_batch(&[])
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn runtime_single_tool() {
    let result = no_perm_runtime(make_registry())
        .execute_single(&empty_call("c1", "read_tool"), &None)
        .await;
    assert!(!result.is_error);
    assert_eq!(result.tool_use_id, "c1");
}

struct StreamingHandler {
    chunks: Vec<String>,
    spec: ToolSpec,
}

impl StreamingHandler {
    fn new(chunks: Vec<String>) -> Self {
        Self {
            spec: ToolSpec::new("stream_tool", "stream", empty_schema()),
            chunks,
        }
    }
}

#[async_trait]
impl ToolHandler for StreamingHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    async fn handle(
        &self,
        _ctx: ToolContext,
        _input: serde_json::Value,
        progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        if let Some(progress) = progress {
            for chunk in &self.chunks {
                let _ = progress.send(crate::contracts::ToolProgress::OutputDelta {
                    delta: chunk.clone(),
                });
            }
        }
        Ok(ToolResult::success(
            ToolResultContent::Text(self.chunks.join("")),
            "done",
        ))
    }
}

fn make_streaming_registry() -> Arc<ToolRegistry> {
    let mut b = ToolRegistryBuilder::new();
    register(
        &mut b,
        "stream_tool",
        Arc::new(StreamingHandler::new(vec!["hello ".into(), "world".into()])),
        spec(
            "stream_tool",
            ToolExecutionMode::Mutating,
            vec![],
            false,
            ToolOutputMode::Text,
        ),
    );
    Arc::new(b.build())
}

#[tokio::test]
async fn execute_single_receives_progress() {
    let result = no_perm_runtime(make_streaming_registry())
        .execute_single(&empty_call("s1", "stream_tool"), &None)
        .await;
    assert!(!result.is_error);
    assert_eq!(result.content.into_string(), "hello world");
}

#[tokio::test]
async fn execute_batch_streaming_receives_progress() {
    let progress_items = Arc::new(std::sync::Mutex::new(Vec::new()));
    let progress_items_for_callback = Arc::clone(&progress_items);
    let results = no_perm_runtime(make_streaming_registry())
        .execute_batch_streaming(
            &[empty_call("s1", "stream_tool")],
            move |tool_use_id, progress| {
                let progress_items_for_callback = Arc::clone(&progress_items_for_callback);
                Box::pin(async move {
                    let ToolProgress::OutputDelta { delta } = progress else {
                        return;
                    };
                    progress_items_for_callback
                        .lock()
                        .expect("progress lock")
                        .push(format!("{tool_use_id}:{delta}"));
                })
            },
        )
        .await;
    assert_eq!(results.len(), 1);
    assert!(!results[0].is_error);
    assert_eq!(results[0].content.clone().into_string(), "hello world");
    assert_eq!(
        *progress_items.lock().expect("progress lock"),
        vec!["s1:hello ".to_string(), "s1:world".to_string()]
    );
}

#[tokio::test]
async fn execute_batch_streaming_empty() {
    assert!(
        no_perm_runtime(make_streaming_registry())
            .execute_batch_streaming(&[], |_, _| Box::pin(async {}))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn execute_batch_streaming_unknown_tool() {
    let results = no_perm_runtime(make_streaming_registry())
        .execute_batch_streaming(
            &[empty_call("x1", "nonexistent")],
            |_, _| Box::pin(async {}),
        )
        .await;
    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedContextOptions {
    output_limit_bytes: usize,
    wall_time_limit_ms: Option<u64>,
    cancel_token_cancelled: bool,
    agent_coordinator_configured: bool,
    agent_scope: ToolAgentScope,
}

struct ContextCaptureTool {
    spec: ToolSpec,
    seen: Arc<std::sync::Mutex<Option<CapturedContextOptions>>>,
}

impl ContextCaptureTool {
    fn new(seen: Arc<std::sync::Mutex<Option<CapturedContextOptions>>>) -> Self {
        Self {
            spec: ToolSpec::new("capture_context", "capture context", empty_schema()),
            seen,
        }
    }
}

#[async_trait]
impl ToolHandler for ContextCaptureTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    async fn handle(
        &self,
        ctx: ToolContext,
        _input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        *self.seen.lock().expect("seen lock") = Some(CapturedContextOptions {
            output_limit_bytes: ctx.budgets.output_limit_bytes,
            wall_time_limit_ms: ctx.budgets.wall_time_limit_ms,
            cancel_token_cancelled: ctx.cancel_token.is_cancelled(),
            agent_coordinator_configured: ctx.agent_coordinator.is_some(),
            agent_scope: ctx.agent_scope,
        });
        Ok(ToolResult::success(
            ToolResultContent::Text("captured".into()),
            "captured",
        ))
    }
}

fn capture_registry(
    seen: &Arc<std::sync::Mutex<Option<CapturedContextOptions>>>,
) -> Arc<ToolRegistry> {
    let mut b = ToolRegistryBuilder::new();
    register(
        &mut b,
        "capture_context",
        Arc::new(ContextCaptureTool::new(Arc::clone(seen))),
        spec(
            "capture_context",
            ToolExecutionMode::ReadOnly,
            vec![],
            true,
            ToolOutputMode::Text,
        ),
    );
    Arc::new(b.build())
}

fn capture_runtime(
    seen: &Arc<std::sync::Mutex<Option<CapturedContextOptions>>>,
    context: ToolRuntimeContext,
    options: ToolExecutionOptions,
) -> ToolRuntime {
    ToolRuntime::new_with_context_and_options(
        capture_registry(seen),
        PermissionChecker::always_allow(),
        context,
        options,
    )
}

fn custom_budgets(cancel_token: CancellationToken) -> ToolExecutionOptions {
    ToolExecutionOptions {
        output_store: None,
        budgets: ToolBudgets {
            output_limit_bytes: 7,
            wall_time_limit_ms: Some(11),
        },
        cancel_token,
        on_tool_execution_start: None,
    }
}

#[tokio::test]
async fn runtime_passes_custom_execution_options_to_tool_context() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let result = capture_runtime(
        &seen,
        ToolRuntimeContext::default(),
        custom_budgets(CancellationToken::new()),
    )
    .execute_single(&empty_call("ctx", "capture_context"), &None)
    .await;
    assert!(!result.is_error);
    assert_eq!(
        *seen.lock().expect("seen lock"),
        Some(CapturedContextOptions {
            output_limit_bytes: 7,
            wall_time_limit_ms: Some(11),
            cancel_token_cancelled: false,
            agent_coordinator_configured: false,
            agent_scope: ToolAgentScope::Parent,
        })
    );
}

#[tokio::test]
async fn runtime_cancels_tool_when_cancel_token_already_fired() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();
    let result = capture_runtime(
        &seen,
        ToolRuntimeContext::default(),
        custom_budgets(cancel_token),
    )
    .execute_single(&empty_call("ctx", "capture_context"), &None)
    .await;
    assert!(result.is_error);
    // The cancel message carries how long the call ran (time-awareness).
    assert!(
        result
            .content
            .into_string()
            .starts_with(INTERRUPTED_TOOL_RESULT_MESSAGE),
        "cancel message must start with the interrupted marker"
    );
    assert!(seen.lock().expect("seen lock").is_none());
}

#[tokio::test]
async fn runtime_interrupts_hanging_tool_via_cancel_token() {
    struct HangingTool {
        spec: ToolSpec,
    }
    #[async_trait]
    impl ToolHandler for HangingTool {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }
        async fn handle(
            &self,
            _ctx: ToolContext,
            _input: serde_json::Value,
            _progress: Option<ToolProgressSender>,
        ) -> Result<ToolResult, ToolCallError> {
            std::future::pending::<()>().await;
            unreachable!("hanging tool should be cancelled")
        }
    }
    let mut b = ToolRegistryBuilder::new();
    register(
        &mut b,
        "hanging",
        Arc::new(HangingTool {
            spec: spec(
                "hanging",
                ToolExecutionMode::ReadOnly,
                vec![],
                true,
                ToolOutputMode::Text,
            ),
        }),
        spec(
            "hanging",
            ToolExecutionMode::ReadOnly,
            vec![],
            true,
            ToolOutputMode::Text,
        ),
    );
    let cancel_token = CancellationToken::new();
    let runtime = ToolRuntime::new_with_context_and_options(
        Arc::new(b.build()),
        PermissionChecker::always_allow(),
        ToolRuntimeContext::default(),
        ToolExecutionOptions {
            cancel_token: cancel_token.clone(),
            ..ToolExecutionOptions::default()
        },
    );
    let hanging_call = empty_call("hang-1", "hanging");
    let execute = runtime.execute_single(&hanging_call, &None);
    tokio::pin!(execute);
    tokio::select! {
        _ = &mut execute => panic!("hanging tool should not complete before cancel"),
        () = tokio::time::sleep(Duration::from_millis(10)) => { cancel_token.cancel(); }
    }
    let result = execute.await;
    assert!(result.is_error);
    assert!(
        result
            .content
            .into_string()
            .starts_with(INTERRUPTED_TOOL_RESULT_MESSAGE),
        "cancel message must start with the interrupted marker"
    );
}

#[derive(Debug, Default)]
struct FakeAgentCoordinator;

#[async_trait]
impl devo_tools::AgentToolCoordinator for FakeAgentCoordinator {
    async fn spawn_agent(
        self: Arc<Self>,
        _params: devo_protocol::SpawnAgentParams,
    ) -> Result<devo_protocol::SpawnAgentResult, ToolCallError> {
        Err(ToolCallError::InternalError("not used".to_string()))
    }
    async fn send_message(
        self: Arc<Self>,
        _params: devo_protocol::AgentMessageParams,
    ) -> Result<devo_protocol::AgentMessageResult, ToolCallError> {
        Err(ToolCallError::InternalError("not used".to_string()))
    }
    async fn wait_agent(
        self: Arc<Self>,
        _params: devo_protocol::WaitAgentParams,
    ) -> Result<devo_protocol::WaitAgentResult, ToolCallError> {
        Err(ToolCallError::InternalError("not used".to_string()))
    }
    async fn list_agents(
        self: Arc<Self>,
        _params: devo_protocol::AgentListParams,
    ) -> Result<Vec<devo_protocol::AgentInfo>, ToolCallError> {
        Err(ToolCallError::InternalError("not used".to_string()))
    }
    async fn close_agent(
        self: Arc<Self>,
        _params: devo_protocol::CloseAgentParams,
    ) -> Result<devo_protocol::CloseAgentResult, ToolCallError> {
        Err(ToolCallError::InternalError("not used".to_string()))
    }
}

#[tokio::test]
async fn runtime_passes_agent_coordinator_to_tool_context() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let result = capture_runtime(
        &seen,
        ToolRuntimeContext {
            agent_coordinator: Some(
                Arc::new(FakeAgentCoordinator) as Arc<dyn devo_tools::AgentToolCoordinator>
            ),
            ..ToolRuntimeContext::default()
        },
        ToolExecutionOptions::default(),
    )
    .execute_single(&empty_call("ctx", "capture_context"), &None)
    .await;
    assert!(!result.is_error);
    assert!(
        seen.lock()
            .expect("seen lock")
            .as_ref()
            .is_some_and(|context| context.agent_coordinator_configured)
    );
}
