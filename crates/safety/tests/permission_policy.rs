use std::path::PathBuf;

use devo_config::PatternMode;
use devo_config::PermissionConfig;
use devo_config::PermissionRule;
use devo_config::RuleAction;
use devo_config::ToolFilter;
use devo_safety::permission::CompiledPolicy;
use devo_safety::permission::PermissionAccess;
use devo_safety::permission::PolicyCompileError;
use devo_safety::permission::PolicyDecision;
use pretty_assertions::assert_eq;
use serde_json::json;

fn rule(
    action: RuleAction,
    tool: ToolFilter,
    pattern: Option<&str>,
    pattern_mode: PatternMode,
) -> PermissionRule {
    PermissionRule {
        action,
        tool,
        pattern: pattern.map(str::to_owned),
        pattern_mode,
    }
}

fn config(rules: Vec<PermissionRule>) -> PermissionConfig {
    PermissionConfig {
        rules,
        ..PermissionConfig::default()
    }
}

fn deny(tool: &str, pattern: Option<&str>) -> PolicyDecision {
    let reason = match pattern {
        Some(pattern) => {
            format!("Denied by permission policy: deny rule on {tool} matching \"{pattern}\"")
        }
        None => format!("Denied by permission policy: deny rule on {tool}"),
    };
    PolicyDecision::Deny { reason }
}

#[test]
fn compile_rejects_invalid_globs_and_domains() {
    let glob_error = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("["),
        PatternMode::Glob,
    )]))
    .expect_err("invalid glob must fail compilation");
    assert_eq!(
        glob_error,
        PolicyCompileError::InvalidGlob {
            rule_index: 0,
            pattern: "[".to_string(),
            message: "unclosed character class; missing ']'".to_string(),
        }
    );

    let domain_error = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Allow,
        ToolFilter::WebFetch,
        Some("bad domain"),
        PatternMode::Domain,
    )]))
    .expect_err("invalid domain must fail compilation");
    assert_eq!(
        domain_error,
        PolicyCompileError::InvalidDomain {
            rule_index: 0,
            pattern: "bad domain".to_string(),
            message: "invalid international domain name".to_string(),
        }
    );

    let wildcard_domain_error = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Allow,
        ToolFilter::WebFetch,
        Some("*.example.com"),
        PatternMode::Domain,
    )]))
    .expect_err("wildcard domain must fail compilation");
    assert_eq!(
        wildcard_domain_error,
        PolicyCompileError::InvalidDomain {
            rule_index: 0,
            pattern: "*.example.com".to_string(),
            message: "domain patterns do not support wildcards".to_string(),
        }
    );
}

#[test]
fn security_precedence_is_independent_of_rule_order() {
    let allow = rule(
        RuleAction::Allow,
        ToolFilter::Bash,
        Some("git *"),
        PatternMode::Glob,
    );
    let ask = rule(
        RuleAction::Ask,
        ToolFilter::Bash,
        Some("git push*"),
        PatternMode::Glob,
    );
    let deny_rule = rule(
        RuleAction::Deny,
        ToolFilter::Bash,
        Some("git push --force*"),
        PatternMode::Glob,
    );
    let access = PermissionAccess::Bash {
        command: "git push --force origin main".to_string(),
        cwd: PathBuf::from("/workspace"),
    };

    for rules in [
        vec![allow.clone(), ask.clone(), deny_rule.clone()],
        vec![deny_rule.clone(), ask.clone(), allow.clone()],
    ] {
        let policy = CompiledPolicy::compile(&config(rules)).expect("compile policy");
        assert_eq!(
            policy.evaluate(&access),
            deny("bash", Some("git push --force*"))
        );
    }

    for rules in [vec![allow.clone(), ask.clone()], vec![ask, allow]] {
        let policy = CompiledPolicy::compile(&config(rules)).expect("compile policy");
        assert_eq!(policy.evaluate(&access), PolicyDecision::Ask);
    }
}

#[test]
fn path_and_tool_filters_match_only_their_access_kinds() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("src/**/*.rs"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    assert_eq!(
        policy.evaluate(&PermissionAccess::Read {
            path: Some(PathBuf::from("src/lib.rs")),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("read", Some("src/**/*.rs"))
    );
    assert_eq!(
        policy.evaluate(&PermissionAccess::Edit {
            path: PathBuf::from("src/lib.rs"),
            cwd: PathBuf::from("/workspace"),
        }),
        PolicyDecision::NoMatch
    );
    assert_eq!(
        policy.evaluate(&PermissionAccess::Read {
            path: None,
            cwd: PathBuf::from("/workspace"),
        }),
        PolicyDecision::NoMatch
    );
}

#[test]
fn read_rules_govern_grep_paths() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("**/.env"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    assert_eq!(
        policy.evaluate(&PermissionAccess::Grep {
            path: Some(PathBuf::from("services/api/.env")),
            glob: Some("*.env".to_string()),
            cwd: PathBuf::from("/workspace"),
            recursive: false,
        }),
        deny("read", Some("**/.env"))
    );
    assert_eq!(
        policy.evaluate(&PermissionAccess::Grep {
            path: None,
            glob: Some("*.env".to_string()),
            cwd: PathBuf::from("/workspace"),
            recursive: false,
        }),
        PolicyDecision::NoMatch
    );
}

#[test]
fn native_file_access_resolves_relative_paths_against_cwd() {
    let read = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("/restricted/**"),
        PatternMode::Glob,
    )]))
    .expect("compile read policy");
    let edit = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Edit,
        Some("/restricted/**"),
        PatternMode::Glob,
    )]))
    .expect("compile edit policy");

    for access in [
        PermissionAccess::Read {
            path: Some(PathBuf::from("../restricted/secret.txt")),
            cwd: PathBuf::from("/workspace"),
        },
        PermissionAccess::Grep {
            path: Some(PathBuf::from("../restricted/secret.txt")),
            glob: None,
            cwd: PathBuf::from("/workspace"),
            recursive: false,
        },
    ] {
        assert_eq!(read.evaluate(&access), deny("read", Some("/restricted/**")));
    }
    assert_eq!(
        edit.evaluate(&PermissionAccess::Edit {
            path: PathBuf::from("../restricted/secret.txt"),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("edit", Some("/restricted/**"))
    );
}

#[cfg(unix)]
#[test]
fn native_and_shell_paths_resolve_dotdot_after_symlinks_physically() {
    use std::os::unix::fs::symlink;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "devo-permission-dotdot-symlink-{}-{nonce}",
        std::process::id(),
    ));
    let workspace = root.join("workspace");
    let outside = root.join("outside");
    std::fs::create_dir_all(outside.join("dir")).expect("create outside tree");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    std::fs::write(outside.join("secret.txt"), "secret").expect("write secret");
    symlink(outside.join("dir"), workspace.join("link")).expect("create symlink");

    let canonical_outside = std::fs::canonicalize(&outside).expect("canonical outside");
    let pattern = format!("{}/**", canonical_outside.to_string_lossy());
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some(&pattern),
        PatternMode::Glob,
    )]))
    .expect("compile policy");
    let expected = deny("read", Some(&pattern));

    assert_eq!(
        policy.evaluate(&PermissionAccess::Read {
            path: Some(PathBuf::from("link/../secret.txt")),
            cwd: workspace.clone(),
        }),
        expected
    );
    assert_eq!(
        policy.evaluate(&PermissionAccess::Bash {
            command: "cat link/../secret.txt".to_string(),
            cwd: workspace,
        }),
        expected
    );

    std::fs::remove_dir_all(root).expect("remove test directory");
}

#[test]
fn domain_matching_normalizes_hosts_and_observes_label_boundaries() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Allow,
        ToolFilter::WebFetch,
        Some("EXAMPLE.COM."),
        PatternMode::Domain,
    )]))
    .expect("compile policy");

    for url in [
        "https://example.com/docs",
        "https://API.Example.Com.:443/docs",
        "http://example.com:80/docs",
    ] {
        assert_eq!(
            policy.evaluate(&PermissionAccess::WebFetch(url.to_string())),
            PolicyDecision::Allow
        );
    }
    assert_eq!(
        policy.evaluate(&PermissionAccess::WebFetch(
            "https://notexample.com/docs".to_string()
        )),
        PolicyDecision::NoMatch
    );
    assert_eq!(
        policy.evaluate(&PermissionAccess::WebFetch(
            "https://example.com.evil.test/docs".to_string()
        )),
        PolicyDecision::NoMatch
    );
}

#[test]
fn mcp_and_web_search_use_owned_devo_native_values() {
    let policy = CompiledPolicy::compile(&config(vec![
        rule(
            RuleAction::Ask,
            ToolFilter::Mcp,
            Some("github__*"),
            PatternMode::Glob,
        ),
        rule(
            RuleAction::Allow,
            ToolFilter::WebSearch,
            Some("rust security"),
            PatternMode::Glob,
        ),
    ]))
    .expect("compile policy");

    assert_eq!(
        policy.evaluate(&PermissionAccess::Mcp {
            name: "github__create_issue".to_string(),
            input: json!({ "title": "Policy" }),
        }),
        PolicyDecision::Ask
    );
    assert_eq!(
        policy.evaluate(&PermissionAccess::WebSearch(
            "rust security advisories".to_string()
        )),
        PolicyDecision::Allow
    );
}

#[test]
fn bash_checks_every_compound_segment_and_common_wrappers() {
    let policy = CompiledPolicy::compile(&config(vec![
        rule(
            RuleAction::Allow,
            ToolFilter::Bash,
            Some("*"),
            PatternMode::Glob,
        ),
        rule(
            RuleAction::Deny,
            ToolFilter::Bash,
            Some("rm *"),
            PatternMode::Glob,
        ),
    ]))
    .expect("compile policy");

    for command in [
        "echo safe && rm -rf build",
        "echo safe; timeout 5 rm -rf build",
        "env MODE=test nice -n 5 rm -rf build",
    ] {
        assert_eq!(
            policy.evaluate(&PermissionAccess::Bash {
                command: command.to_string(),
                cwd: PathBuf::from("/workspace"),
            }),
            deny("bash", Some("rm *"))
        );
    }
}

#[test]
fn bash_recurses_into_nested_shell_dash_c_scripts() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Bash,
        Some("id"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    for command in [
        "bash -c 'echo safe; id'",
        "timeout 5 sh -c -- 'id > result.txt'",
    ] {
        assert_eq!(
            policy.evaluate(&PermissionAccess::Bash {
                command: command.to_string(),
                cwd: PathBuf::from("/workspace"),
            }),
            deny("bash", Some("id"))
        );
    }
}

#[test]
fn shell_redirections_and_writer_paths_resolve_against_cwd() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Edit,
        Some("/workspace/private/**"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    for command in [
        "echo secret > private/output.txt",
        "printf secret | tee private/output.txt",
        "touch private/output.txt",
        "bash -c 'echo nested > private/output.txt'",
    ] {
        assert_eq!(
            policy.evaluate(&PermissionAccess::Bash {
                command: command.to_string(),
                cwd: PathBuf::from("/workspace"),
            }),
            deny("edit", Some("/workspace/private/**"))
        );
    }

    let escaped_policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Edit,
        Some("/private/**"),
        PatternMode::Glob,
    )]))
    .expect("compile traversal policy");
    assert_eq!(
        escaped_policy.evaluate(&PermissionAccess::Bash {
            command: "echo secret > ../../private/output.txt".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("edit", Some("/private/**"))
    );
}

#[test]
fn shell_redirects_follow_literal_cwd_changes_in_source_order() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Edit,
        Some("/restricted/**"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    for command in [
        "cd /restricted && echo secret > secret.txt",
        "cd -- /restricted && echo secret > secret.txt",
        "cd ../restricted && echo secret > secret.txt",
        "echo safe > before.txt && cd /restricted && echo secret > secret.txt",
        "cd /restricted > before.txt && echo secret > secret.txt",
    ] {
        assert_eq!(
            policy.evaluate(&PermissionAccess::Bash {
                command: command.to_string(),
                cwd: PathBuf::from("/workspace"),
            }),
            deny("edit", Some("/restricted/**")),
            "command: {command}"
        );
    }

    assert_eq!(
        policy.evaluate(&PermissionAccess::Bash {
            command: "echo safe > before.txt && cd /restricted".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        PolicyDecision::NoMatch
    );
    assert_eq!(
        policy.evaluate(&PermissionAccess::Bash {
            command: "cd \"$TARGET\" && echo secret > secret.txt".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        PolicyDecision::Ask
    );
}

#[test]
fn shell_readers_cannot_bypass_read_rules() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("**/.env"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    assert_eq!(
        policy.evaluate(&PermissionAccess::Bash {
            command: "cat services/api/.env".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("read", Some("**/.env"))
    );
}

#[test]
fn shell_searches_and_pattern_files_are_inferred_as_reads() {
    let cwd_policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("/workspace"),
        PatternMode::Glob,
    )]))
    .expect("compile cwd policy");
    assert_eq!(
        cwd_policy.evaluate(&PermissionAccess::Bash {
            command: "rg needle".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("read", Some("/workspace"))
    );

    let pattern_policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("**/.env"),
        PatternMode::Glob,
    )]))
    .expect("compile pattern-file policy");
    assert_eq!(
        pattern_policy.evaluate(&PermissionAccess::Bash {
            command: "grep -f .env README.md".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("read", Some("**/.env"))
    );
}

#[test]
fn recursive_shell_searches_enforce_descendant_read_denies() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("**/.env"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    for command in [
        "rg needle .",
        "rg needle",
        "rg needle services",
        "grep -r needle .",
        "grep -R needle services",
        "grep --recursive needle .",
    ] {
        assert_eq!(
            policy.evaluate(&PermissionAccess::Bash {
                command: command.to_string(),
                cwd: PathBuf::from("/workspace"),
            }),
            deny("read", Some("**/.env")),
            "command: {command}"
        );
    }

    let grep_policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Grep,
        Some("**/.env"),
        PatternMode::Glob,
    )]))
    .expect("compile grep policy");
    assert_eq!(
        grep_policy.evaluate(&PermissionAccess::Bash {
            command: "rg needle .".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("grep", Some("**/.env"))
    );
}

#[test]
fn bash_rules_normalize_executables_and_peel_execution_wrappers() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Bash,
        Some("rm *"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    for command in [
        "command rm -rf build",
        "exec rm -rf build",
        "/bin/rm -rf build",
        "command exec /bin/rm -rf build",
        "timeout 5 command /bin/rm -rf build",
    ] {
        assert_eq!(
            policy.evaluate(&PermissionAccess::Bash {
                command: command.to_string(),
                cwd: PathBuf::from("/workspace"),
            }),
            deny("bash", Some("rm *")),
            "command: {command}"
        );
    }
}

#[test]
fn external_shell_scripts_ask_under_bash_restrictions() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Bash,
        Some("rm *"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    for command in [
        "bash script.sh",
        "sh ./x",
        "command /bin/bash script.sh",
        "exec sh ./x",
        "timeout 5 command exec /bin/sh ./x",
    ] {
        assert_eq!(
            policy.evaluate(&PermissionAccess::Bash {
                command: command.to_string(),
                cwd: PathBuf::from("/workspace"),
            }),
            PolicyDecision::Ask,
            "command: {command}"
        );
    }
}

#[test]
fn in_place_sed_is_both_a_read_and_an_edit() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("**/secret.txt"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    assert_eq!(
        policy.evaluate(&PermissionAccess::Bash {
            command: "sed -i 's/old/new/' private/secret.txt".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("read", Some("**/secret.txt"))
    );
}

#[test]
fn unmodeled_shell_commands_fail_closed_only_with_file_restrictions() {
    let read_restricted = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("**/.env"),
        PatternMode::Glob,
    )]))
    .expect("compile read policy");
    let edit_restricted = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Edit,
        Some("**/secret.txt"),
        PatternMode::Glob,
    )]))
    .expect("compile edit policy");
    let unrestricted =
        CompiledPolicy::compile(&PermissionConfig::default()).expect("compile empty policy");

    for command in ["python script.py", "unknown-tool input.txt"] {
        let access = PermissionAccess::Bash {
            command: command.to_string(),
            cwd: PathBuf::from("/workspace"),
        };
        assert_eq!(read_restricted.evaluate(&access), PolicyDecision::Ask);
        assert_eq!(edit_restricted.evaluate(&access), PolicyDecision::Ask);
        assert_eq!(unrestricted.evaluate(&access), PolicyDecision::NoMatch);
    }

    for command in ["echo safe", "printf safe", "pwd", "true"] {
        let access = PermissionAccess::Bash {
            command: command.to_string(),
            cwd: PathBuf::from("/workspace"),
        };
        assert_eq!(read_restricted.evaluate(&access), PolicyDecision::NoMatch);
        assert_eq!(edit_restricted.evaluate(&access), PolicyDecision::NoMatch);
    }
}

#[test]
fn shell_move_and_output_placement_are_modeled() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Edit,
        Some("/workspace/restricted/**"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    for command in [
        "mv restricted/source.txt /tmp/destination.txt",
        "cp -t restricted source.txt",
        "mv --target-directory=restricted source.txt",
        "sort --output=restricted/sorted.txt input.txt",
    ] {
        assert_eq!(
            policy.evaluate(&PermissionAccess::Bash {
                command: command.to_string(),
                cwd: PathBuf::from("/workspace"),
            }),
            deny("edit", Some("/workspace/restricted/**")),
            "command: {command}"
        );
    }

    let read_policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some("/workspace/restricted/**"),
        PatternMode::Glob,
    )]))
    .expect("compile read policy");
    assert_eq!(
        read_policy.evaluate(&PermissionAccess::Bash {
            command: "mv restricted/source.txt /tmp/destination.txt".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("read", Some("/workspace/restricted/**"))
    );

    let out_dir_policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Edit,
        Some("/workspace/restricted"),
        PatternMode::Glob,
    )]))
    .expect("compile out-dir policy");
    assert_eq!(
        out_dir_policy.evaluate(&PermissionAccess::Bash {
            command: "rustc src/main.rs --out-dir restricted".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        deny("edit", Some("/workspace/restricted"))
    );
}

#[cfg(unix)]
#[test]
fn shell_paths_are_rechecked_after_following_symlinks() {
    use std::os::unix::fs::symlink;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "devo-permission-symlink-{}-{nonce}",
        std::process::id()
    ));
    let restricted = root.join("restricted");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&restricted).expect("create restricted dir");
    std::fs::create_dir_all(&workspace).expect("create workspace dir");
    std::fs::write(restricted.join("secret.txt"), "secret").expect("write secret");
    symlink(&restricted, workspace.join("alias")).expect("create symlink");

    let canonical_restricted = std::fs::canonicalize(&restricted).expect("canonical restricted");
    let pattern = format!("{}/**", canonical_restricted.to_string_lossy());
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Read,
        Some(&pattern),
        PatternMode::Glob,
    )]))
    .expect("compile symlink policy");
    assert_eq!(
        policy.evaluate(&PermissionAccess::Bash {
            command: "cat alias/secret.txt".to_string(),
            cwd: workspace,
        }),
        deny("read", Some(&pattern))
    );

    std::fs::remove_dir_all(root).expect("remove test directory");
}

#[test]
fn malformed_shell_fails_closed_and_modeled_shell_does_not_ask() {
    let restricted = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Edit,
        Some("**/.env"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");
    let unrestricted =
        CompiledPolicy::compile(&PermissionConfig::default()).expect("compile empty policy");
    let bash_restricted = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Deny,
        ToolFilter::Bash,
        Some("rm *"),
        PatternMode::Glob,
    )]))
    .expect("compile bash policy");
    let access = PermissionAccess::Bash {
        command: "echo $(cat .env".to_string(),
        cwd: PathBuf::from("/workspace"),
    };

    assert_eq!(restricted.evaluate(&access), PolicyDecision::Ask);
    assert_eq!(bash_restricted.evaluate(&access), PolicyDecision::Ask);
    assert_eq!(unrestricted.evaluate(&access), PolicyDecision::NoMatch);

    let supported = PermissionAccess::Bash {
        command: "if true; then echo safe; fi".to_string(),
        cwd: PathBuf::from("/workspace"),
    };
    assert_eq!(
        bash_restricted.evaluate(&supported),
        PolicyDecision::NoMatch
    );
    assert_eq!(unrestricted.evaluate(&supported), PolicyDecision::NoMatch);
}

#[test]
fn file_allow_rules_do_not_auto_allow_a_bash_command() {
    let policy = CompiledPolicy::compile(&config(vec![rule(
        RuleAction::Allow,
        ToolFilter::Edit,
        Some("/workspace/out.txt"),
        PatternMode::Glob,
    )]))
    .expect("compile policy");

    assert_eq!(
        policy.evaluate(&PermissionAccess::Bash {
            command: "echo ok > out.txt".to_string(),
            cwd: PathBuf::from("/workspace"),
        }),
        PolicyDecision::NoMatch
    );
}
