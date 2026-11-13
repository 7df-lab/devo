//! Immutable RLM system-prompt doctrine for Devo sessions.
//!
//! Kernel-surface RLM prompt set (reference-agent design, Devo branding) with explicit
//! bounded subagent and independent-root controls. Catalog skill progressive disclosure
//! uses XML (`format_catalog_skills_xml`); the markdown skill body from
//! `devo_skills::render_available_skills_body` remains for non-RLM surfaces.

use std::borrow::Cow;

const REPL_CONTROL_PROMPT: &str = include_str!("../prompts/rlm/repl_control.md");

const LONG_RUNNING_WORK_PROMPT: &str = concat!(
    "For slow or independently completing work, use a nonblocking control loop: ",
    "start the work, record its handle or output location, then end your turn. ",
    "A `bash()` handle left running beyond its creating cell sends a completion ",
    "follow-up; when it arrives, inspect the saved handle and continue. Reading a ",
    "finished handle's result first cancels that follow-up.\n",
    "Long synchronous Python cells may be parked by the harness after a wait budget; ",
    "prefer `bash()` for subprocesses that should run independently of the REPL. ",
    "A parked Python cell also sends a completion follow-up when it finishes.\n",
    "When delegation is available and useful, assign independent substantive tasks ",
    "to separate workers. Start independent workers without waiting for each one ",
    "sequentially, and let them run in parallel.\n",
    "Do not keep the turn open by polling with `time.sleep()` or shell `sleep`, and ",
    "do not replace polling with a long blocking `await`. Await only the short ",
    "operation needed to start work or inspect a result that is already available; ",
    "otherwise end the turn."
);

const USER_PROGRESS_PROMPT: &str = concat!(
    "As the user-facing root agent, when work follows a plan, uses many subagents, ",
    "or spans multiple turns, proactively give regular concise progress updates so ",
    "the user does not have to ask. State the current plan, what has completed, any ",
    "blockers, the proposed fixes, and the next actions. Lead with user-visible ",
    "outcomes rather than internal process or gate names. Mention internal details ",
    "only when they explain a blocker or decision. Send an update at meaningful ",
    "milestones and before ending a turn while work is still running. Do not repeat ",
    "unchanged status or interrupt short work with unnecessary updates."
);

pub(crate) const LANGUAGE_PREFERENCE_PROMPT: &str = "Follow the user's requested language; otherwise use the primary natural language of their latest message, including plans and goals. Preserve technical terms, code identifiers, file paths, commands, API names, and quoted text unless the user explicitly asks to translate them. Keep <proposed_plan> and <objective> tags unchanged while writing their content in the chosen language.";

const CLEAR_LANGUAGE_PROMPT: &str = concat!(
    "Prefer short sentences, common words, and concrete verbs. State one main ",
    "action or fact per sentence when practical. Use lists for steps or conditions.\n",
    "Keep necessary technical terms, names, commands, code, paths, and exact quoted ",
    "text unchanged. State uncertainty directly.\n",
    "Apply this clarity guidance in the chosen language. ",
    "Preserve a user-requested format, tone, terminology, and necessary precision."
);

/// Pre-installed scientific / utility packages advertised in the RLM base prompt.
pub const DEFAULT_RLM_EXTRA_IMPORT_LABELS: &[&str] = &[
    "requests",
    "httpx",
    "yaml (PyYAML)",
    "tomli",
    "dotenv (python-dotenv)",
    "pandas",
    "numpy",
    "scipy",
    "bs4 (Beautiful Soup)",
    "lxml",
    "pydantic",
    "tyro",
];

/// Options for [`build_rlm_base_prompt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RlmPromptOptions {
    pub cwd: String,
    pub messages_path: String,
    pub skills_dir: Option<String>,
    pub installed_skills: Vec<String>,
    pub allow_recursion: bool,
    pub depth: u32,
    pub parent_agent: Option<String>,
    /// When `None`, assume the `ipython` tool is available (shipped RLM default).
    pub active_tools: Option<Vec<String>>,
}

impl RlmPromptOptions {
    pub fn root(cwd: impl Into<String>, messages_path: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            messages_path: messages_path.into(),
            skills_dir: None,
            installed_skills: Vec::new(),
            allow_recursion: true,
            depth: 0,
            parent_agent: None,
            active_tools: None,
        }
    }

    pub fn child(
        cwd: impl Into<String>,
        messages_path: impl Into<String>,
        depth: u32,
        parent_agent: impl Into<String>,
    ) -> Self {
        Self {
            depth,
            parent_agent: Some(parent_agent.into()),
            ..Self::root(cwd, messages_path)
        }
    }

    fn has_ipython(&self) -> bool {
        match &self.active_tools {
            None => true,
            Some(tools) => tools.iter().any(|tool| tool == "ipython"),
        }
    }

    fn has_skill(&self, name: &str) -> bool {
        self.installed_skills.iter().any(|skill| skill == name)
    }
}

/// Catalog skill metadata for XML progressive disclosure (name / description / location).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSkillEntry {
    pub name: String,
    pub description: String,
    pub location: String,
    pub python_import: Option<String>,
}

/// Build the immutable Devo RLM base system prompt (root or child).
pub fn build_rlm_base_prompt(options: &RlmPromptOptions) -> String {
    let has_ipython = options.has_ipython();
    let has_agent_message = options.has_skill("agent_message");
    let has_agent_observe = options.has_skill("agent_observe");
    let can_run_shell_skills = has_ipython
        || options
            .active_tools
            .as_ref()
            .is_some_and(|tools| tools.iter().any(|tool| tool == "bash"));

    let mut parts: Vec<String> = vec![
        "You are Devo, a coding agent that uses a persistent Python REPL (RLM) to solve tasks."
            .to_string(),
        "You solve tasks by breaking down problems into sub-tasks, writing and executing code, observing results, and iterating one step at a time."
            .to_string(),
        "When you are done, stop calling tools and state your final answer.".to_string(),
        String::new(),
        LONG_RUNNING_WORK_PROMPT.to_string(),
        String::new(),
    ];

    if options.depth == 0 {
        parts.push(USER_PROGRESS_PROMPT.to_string());
        parts.push(String::new());
    }

    parts.push(LANGUAGE_PREFERENCE_PROMPT.to_string());
    parts.push(CLEAR_LANGUAGE_PROMPT.to_string());
    parts.push(String::new());
    parts.push(format!("Working directory: {}", options.cwd));
    parts.push(format!("Conversation log: {}", options.messages_path));
    parts.push(format!("Recursive agent depth: {}", options.depth));
    parts.push(format!(
        "Pre-installed Python packages: {}.",
        DEFAULT_RLM_EXTRA_IMPORT_LABELS.join(", ")
    ));
    parts.push(
        "Install additional packages with `uv pip install <pkg>` (this is a uv-managed venv with no pip module)."
            .to_string(),
    );

    if has_ipython {
        parts.push(String::new());
        parts.push(
            "The Python kernel is a trusted orchestration process with user-level OS access, not a sandbox for untrusted code. Plan mode does not OS-confine Python; respect its no-mutation policy. Use host-mediated APIs for shell execution and approval requests. Never use direct Python process or filesystem APIs to bypass a denied host operation."
                .to_string(),
        );
    }

    if can_run_shell_skills {
        parts.push(String::new());
        parts.push(
            "Sandbox and approvals: Python `bash()` requests host-owned shell execution under the session's configured sandbox profile and approval policy. For additional access, use `await rlm.shell(command, sandbox_permissions=\"with_additional_permissions\", additional_permissions={\"network\": {\"enabled\": True}}, justification=\"May I ...?\")` for narrow grants, or `await rlm.shell(command, sandbox_permissions=\"require_escalated\", justification=\"May I ...?\")` for an approved unsandboxed command. Denial is final; do not retry through another route. Never attach these fields to an `ipython` call itself. Commands default to Bash on Unix and PowerShell on Windows; use syntax for the host platform. `rlm.shell` returns a dict with command output and status."
                .to_string(),
        );
    }

    if let Some(child_doctrine) = build_child_agent_doctrine(options) {
        parts.push(String::new());
        parts.push(child_doctrine);
    }

    let mut skill_lines: Vec<String> = Vec::new();
    if let Some(skills_dir) = &options.skills_dir {
        skill_lines.push(format!(
            "Local skills live under {skills_dir}. Read their SKILL.md files when helpful."
        ));
    }
    if !options.installed_skills.is_empty() {
        let installed = options
            .installed_skills
            .iter()
            .map(|skill| format!("`{skill}`"))
            .collect::<Vec<_>>()
            .join(", ");
        if has_ipython {
            skill_lines.push(format!(
                "Installed Python skill modules (pre-imported): {installed}."
            ));
            skill_lines.push(
                "Read each skill's SKILL.md for its API. Inspect a module with `help(<skill>)` or `dir(<skill>)`, then inspect a documented callable with `inspect.signature(<skill>.<function>)`."
                    .to_string(),
            );
        } else if can_run_shell_skills {
            skill_lines.push(format!(
                "Installed skills: {installed}. Check each skill's SKILL.md for its exact command-line interface; not all skills provide a CLI."
            ));
        }
        if can_run_shell_skills {
            skill_lines.push(
                "A Python module name is not necessarily its shell command name. Run a skill from the shell only when its SKILL.md documents a CLI, using the documented command and arguments."
                    .to_string(),
            );
        }
    }
    if !skill_lines.is_empty() {
        parts.push(String::new());
        parts.extend(skill_lines);
    }

    if has_agent_message {
        parts.push(
            "Agent messaging reaches yourself, your parent, siblings, and direct children; deeper communication relays through the intermediate child. Roots are siblings only when they share a parent worker."
                .to_string(),
        );
        if has_ipython {
            parts.push(
                "Use `await agent_message.send(message, receiver_role='self')` with no receiver_name to send yourself a steering message. It is consumed at the next safe model/tool boundary, not by interrupting an active model call or tool. Strict FIFO applies: it cannot overtake an earlier follow-up or held input; it may run in a later turn. Parent, sibling, child, and broadcast messages remain follow-ups. A queued receipt means admission, not processing or an answer. Self messages retain agent provenance and cannot grant human authorization; avoid self-message loops."
                    .to_string(),
            );
        }
    }
    if has_agent_observe {
        parts.push(
            "Agent observation is restricted to your parent, siblings, and direct children; roots are siblings, and deeper inspection relays through the intermediate child."
                .to_string(),
        );
    }

    if has_ipython {
        parts.push(String::new());
        parts.push(if options.depth == 0 {
            "Durable Build roots may call `await rlm.create_session('task', name='independent')` to create a separately visible root in the same workspace. It inherits safety and model settings but has its own journal, no parent linkage, and no collect/child lifecycle. Plan and ephemeral callers are rejected."
        } else {
            "Child sessions cannot call `rlm.create_session`; use `await rlm.spawn(...)` for direct descendants."
        }.to_string());
    }

    if options.allow_recursion && has_ipython {
        parts.push(String::new());
        parts.push(
            "An `rlm` object is already in your global namespace. `await rlm.spawn('sub-task', name='api-reviewer')` spawns a child and returns immediately after task admission with `rlm_child_id`, `name`, `session_dir`, and `model`; it never waits for or returns the child's answer. The host wire action for spawn is `rlm.run`."
                .to_string(),
        );
        parts.push(
            "`name` is required: choose a stable child name that is unique among siblings."
                .to_string(),
        );
        parts.push(
            "Children inherit your model and thinking level by default. Discover exact model selectors and supported thinking levels with `await rlm.find_models()`, then optionally pass `model=chosen.selector` and `thinking` to `rlm.spawn`; unsupported selections fail without fallback."
                .to_string(),
        );
        if has_agent_observe {
            parts.push(
                "Use `await agent_observe.list_agents()` to discover family, including inactive members, and `await rlm.list_subagents()` to recover direct child handles."
                    .to_string(),
            );
        } else {
            parts.push(
                "Use `await rlm.list_subagents()` to recover direct child handles after admission."
                    .to_string(),
            );
        }
        if has_agent_message {
            parts.push(
                "Children report final results in assistant output, which is included in their completion record. For timely progress or a direct reply, use `await agent_message.send(message, receiver_role='parent')`; these arrive as ordinary agent messages."
                    .to_string(),
            );
            parts.push(
                "Use `agent_message.send(..., receiver_role='child', receiver_name=child.name)` for follow-ups."
                    .to_string(),
            );
        }
        if has_agent_observe {
            parts.push(
                "Use `agent_observe` to inspect a child's rollout. Observation is restricted to your parent, siblings, and direct children; relay through the intermediate child for deeper descendants."
                    .to_string(),
            );
        } else {
            parts.push(
                "Use `await rlm.collect(child, timeout_ms=0)` to inspect retained child results without an observation capability, or read the files the child wrote."
                    .to_string(),
            );
        }
        parts.push(
            "Use `await rlm.collect(children, timeout_ms=1000)` for a short bounded wait: timeout returns partial snapshots without cancellation. Check `settled`, `requires_resume`, and `answer_truncated`; saved unfinished work requires explicit recovery, not automatic replay. Children can report `await rlm.progress_note('running tests')` without steering the parent. `await rlm.end_turn()` ends normally after the current tool batch while children continue. Delete a direct child explicitly with `await rlm.delete_subagent(child)` when it is no longer needed."
                .to_string(),
        );
    }

    if has_ipython {
        parts.push(String::new());
        parts.push(REPL_CONTROL_PROMPT.trim_end().to_string());
        if options.depth == 0 && options.has_skill("refine") {
            parts.push(String::new());
            parts.push(
                "Treat continual harness refinement as a small, evidence-backed update after observing a repeated failure or reusable tactic: diagnose the issue, update the smallest relevant continual harness component, validate on the next action, then record the outcome. Use `await refine.run()` to turn repeated delegation patterns into reusable subagent specs, repeated procedures into skills, durable facts/preferences into memories, and narrow behavioral policies into prompt addendums. It returns immediately and runs when the current turn ends, so continue working normally after calling it. Do not rewrite the whole continual harness when a focused memory, skill, prompt note, or subagent spec is enough."
                    .to_string(),
            );
        }
    }

    parts.join("\n")
}

/// Child-only immutable doctrine (own kernel, parent messaging, no auto-refine).
pub fn build_child_agent_doctrine(options: &RlmPromptOptions) -> Option<String> {
    if options.depth == 0 {
        return None;
    }

    let parent = options
        .parent_agent
        .as_deref()
        .unwrap_or("your parent agent");
    let mut lines = vec![
        format!(
            "You are a child agent spawned by {parent}. Your delegated task is provided in the session input."
        ),
        "You run in your own RLM kernel namespace; do not assume shared variables with the parent."
            .to_string(),
        "Permission, sandbox, and MCP inheritance never relax relative to the parent."
            .to_string(),
        "Auto-refine is disabled for child sessions (including `/btw`); the host rejects child `refine.run` calls."
            .to_string(),
        "Ephemeral children, including `/btw`, have no persistent local harness store: local loads return empty state and local writes are rejected. They are cascade-deleted with the parent."
            .to_string(),
    ];

    if options.has_skill("agent_message") && options.has_ipython() {
        lines.push(
            "Report final results in assistant output for the completion record. Send useful progress or time-sensitive answers with `await agent_message.send(message, receiver_role=\"parent\")`; do not send redundant acknowledgments."
                .to_string(),
        );
    } else {
        lines.push(
            "Report final results in assistant output for the completion record. Save substantial artifacts to files the parent can read; do not call unavailable messaging skills."
                .to_string(),
        );
    }

    Some(lines.join("\n"))
}

/// Format catalog skills as XML progressive disclosure (name / description / location).
///
/// Compatibility helper for callers that need XML. The active Native prompt
/// pipeline uses [`devo_skills::render_available_skills_body`] instead.
pub fn format_catalog_skills_xml(skills: &[CatalogSkillEntry]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        String::new(),
        "The following skills provide specialized instructions for specific tasks.".to_string(),
        "Use ipython to inspect a skill's file when the task matches its description.".to_string(),
        "Skills with a python_import are prepared in the persistent Python kernel when available and can be called directly by that import name.".to_string(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];

    for skill in skills {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        if let Some(python_import) = &skill.python_import {
            lines.push("    <type>python</type>".to_string());
            lines.push(format!(
                "    <python_import>{}</python_import>",
                escape_xml(python_import)
            ));
        }
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.location)
        ));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

fn escape_xml(text: &str) -> Cow<'_, str> {
    let Some(first_escape_at) = text.find(['&', '<', '>', '"', '\'']) else {
        return Cow::Borrowed(text);
    };

    let mut escaped = String::with_capacity(text.len());
    escaped.push_str(&text[..first_escape_at]);
    for ch in text[first_escape_at..].chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    Cow::Owned(escaped)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// Root prompt teaches the supported controls and their safety boundaries.
    #[test]
    fn build_rlm_base_prompt_contains_key_doctrine() {
        let mut options = RlmPromptOptions::root("/repo", "/tmp/messages.jsonl");
        options.installed_skills = vec!["refine".into(), "goal".into()];
        let prompt = build_rlm_base_prompt(&options);

        assert!(prompt.contains("You are Devo"));
        assert!(prompt.contains("ipython"));
        assert!(prompt.contains("bash()"));
        assert!(prompt.contains("await rlm.shell(command"));
        assert!(prompt.contains("Never attach these fields to an `ipython` call"));
        assert!(!prompt.contains("Request it on the tool call itself"));
        assert!(prompt.contains("16 MiB"));
        assert!(prompt.contains("rlm.spawn"));
        assert!(prompt.contains("rlm.harness"));
        assert!(prompt.contains("rlm.create_session"));
        assert!(prompt.contains("Durable Build roots"));
        assert!(prompt.contains("rlm.collect"));
        assert!(prompt.contains("rlm.progress_note"));
        assert!(prompt.contains("rlm.end_turn"));
        assert!(!prompt.contains("Daemon-backed top-level session creation is denied"));
        assert!(!prompt.contains("daemon-backed depth-0 session"));
        assert!(prompt.contains("Recursive agent depth: 0"));
        assert!(prompt.contains("await refine.run()"));
    }

    #[test]
    fn bash_guidance_requires_a_python_cell_for_root_and_child() {
        for mut options in [
            RlmPromptOptions::root("/repo", "/tmp/messages.jsonl"),
            RlmPromptOptions::child("/repo", "/tmp/messages.jsonl", 1, "parent"),
        ] {
            for active_tools in [None, Some(vec!["ipython".to_string()])] {
                options.active_tools = active_tools;
                let prompt = build_rlm_base_prompt(&options);
                assert!(prompt.contains("Inside an `ipython` Python cell"));
                assert!(prompt.contains("Neither is a standalone model tool"));
                assert!(
                    prompt.contains("`r = await rlm.shell(command)` for short one-shot commands")
                );
                assert!(prompt.contains("first get a handle with `h = bash(command)`"));
                assert!(prompt.contains("then use `r = await h`"));
                assert!(prompt.contains("Native sessions return a dict with `stdout` and `exit`"));
                assert!(!prompt.contains("await bash("));
                assert!(prompt.contains(
                    r#"`h = bash(command, mode="interactive")` for ongoing input/output"#
                ));
                assert_eq!(
                    prompt.matches("Neither is a standalone model tool").count(),
                    1
                );
            }
            for active_tools in [vec![], vec!["bash".to_string()]] {
                options.active_tools = Some(active_tools);
                let prompt = build_rlm_base_prompt(&options);
                assert!(!prompt.contains("Neither is a standalone model tool"));
            }
        }
    }

    #[test]
    fn built_in_python_skills_are_not_assumed_to_be_shell_commands() {
        let mut options = RlmPromptOptions::root("/repo", "/tmp/messages.jsonl");
        options.installed_skills = vec!["refine".into(), "goal".into()];
        let prompt = build_rlm_base_prompt(&options);

        for import in &options.installed_skills {
            assert!(
                prompt.contains(&format!("`{import}`")),
                "missing skill import {import}"
            );
        }
        assert!(prompt.contains("Installed Python skill modules (pre-imported)"));
        assert!(prompt.contains("A Python module name is not necessarily its shell command name"));
        assert!(prompt.contains("only when its SKILL.md documents a CLI"));
        assert!(!prompt.contains("Each skill is also available as a shell command"));
    }

    /// Verifies: child prompt differs from root (parent messaging, no auto-refine, own kernel).
    #[test]
    fn child_prompt_differs_from_root() {
        let root = build_rlm_base_prompt(&RlmPromptOptions::root("/repo", "/tmp/messages.jsonl"));
        let mut child_opts =
            RlmPromptOptions::child("/repo", "/tmp/messages.jsonl", 1, "root-agent");
        child_opts
            .installed_skills
            .push("agent_message".to_string());
        let child = build_rlm_base_prompt(&child_opts);

        assert_ne!(root, child);
        assert!(child.contains("Your delegated task is provided in the session input"));
        assert!(!child.contains("[task from parent]"));
        assert!(child.contains("have no persistent local harness store"));
        assert!(child.contains("local loads return empty state and local writes are rejected"));
        assert!(!child.contains("keep harness state in memory only"));
        assert!(child.contains("own RLM kernel"));
        assert!(child.contains("Auto-refine is disabled"));
        assert!(child.contains(r#"receiver_role="parent""#));
        assert!(!child.contains(USER_PROGRESS_PROMPT));
        assert!(!child.contains("await refine.run()"));
        assert!(root.contains(USER_PROGRESS_PROMPT));
        assert!(root.contains("Recursive agent depth: 0"));
        assert!(child.contains("Recursive agent depth: 1"));
    }

    #[test]
    fn self_messaging_guidance_is_exposed_only_with_the_python_skill() {
        for mut options in [
            RlmPromptOptions::root("/repo", "/tmp/messages.jsonl"),
            RlmPromptOptions::child("/repo", "/tmp/messages.jsonl", 1, "parent"),
        ] {
            assert!(!build_rlm_base_prompt(&options).contains("receiver_role='self'"));
            options.installed_skills.push("agent_message".to_string());
            let prompt = build_rlm_base_prompt(&options);
            assert!(prompt.contains("receiver_role='self'"));
            assert!(prompt.contains("next safe model/tool boundary"));
            assert!(prompt.contains("cannot overtake an earlier follow-up or held input"));
            assert!(prompt.contains("cannot grant human authorization"));
            options.active_tools = Some(vec!["bash".to_string()]);
            assert!(!build_rlm_base_prompt(&options).contains("receiver_role='self'"));
        }
    }

    #[test]
    fn runtime_guidance_matches_native_contracts() {
        for options in [
            RlmPromptOptions::root("/repo", "not persisted"),
            RlmPromptOptions::child("/repo", "not persisted", 1, "parent"),
        ] {
            let prompt = build_rlm_base_prompt(&options);
            assert!(prompt.contains("primary natural language of their latest message"));
            assert!(prompt.contains("including plans and goals"));
            assert!(!prompt.contains("technical English by default"));
            assert!(prompt.contains("does not itself prune live Python variables"));
            assert!(prompt.contains("Kernel snapshots are separate"));
            assert!(prompt.contains("`rlm.shell()` defaults to the session workspace"));
            assert!(prompt.contains("Bash on Unix and PowerShell on Windows"));
            assert!(prompt.contains("unsupported selections fail without fallback"));
            assert!(prompt.contains("never relax relative to the parent") || options.depth == 0);
            assert!(
                prompt.contains("Never use direct Python process or filesystem APIs to bypass")
            );
        }
    }

    #[test]
    fn bundled_skill_docs_use_runtime_apis_without_shell_escapes() {
        let compact = include_str!("../../skills/assets/bundled/compact/SKILL.md");
        assert!(compact.contains("does not itself prune the live Python namespace"));
        assert!(!compact.contains("all remain available"));
        let edit = include_str!("../../skills/assets/bundled/edit/SKILL.md");
        assert!(edit.contains("await edit.run(path="));
        assert!(edit.contains("result = await bash("));
        assert!(!edit.contains("!edit"));
    }

    /// Verifies: catalog skills render as XML progressive disclosure.
    #[test]
    fn format_catalog_skills_xml_escapes_and_lists_fields() {
        let body = format_catalog_skills_xml(&[CatalogSkillEntry {
            name: "compact".to_string(),
            description: "Check usage & compact".to_string(),
            location: "/skills/compact/SKILL.md".to_string(),
            python_import: Some("compact".to_string()),
        }]);

        assert!(body.contains("<available_skills>"));
        assert!(body.contains("<name>compact</name>"));
        assert!(body.contains("<description>Check usage &amp; compact</description>"));
        assert!(body.contains("<location>/skills/compact/SKILL.md</location>"));
        assert!(body.contains("<python_import>compact</python_import>"));
        assert_eq!(format_catalog_skills_xml(&[]), "");
    }
}
