use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;

use devo_protocol::ToolDefinition;

use crate::tool_spec::ToolSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptLoadingPolicy {
    Preloaded,
    Deferred,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredLoadingConfig {
    pub enabled: bool,
    pub default_policy: PromptLoadingPolicy,
    pub preloaded: BTreeSet<String>,
    pub deferred: BTreeSet<String>,
    pub hidden: BTreeSet<String>,
}

impl Default for DeferredLoadingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_policy: PromptLoadingPolicy::Deferred,
            preloaded: default_preloaded_tools(),
            deferred: default_deferred_tools(),
            hidden: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredTool {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchMetrics {
    pub baseline_tool_schema_tokens: usize,
    pub exposed_tool_schema_tokens: usize,
    pub deferred_reminder_tokens: usize,
    pub estimated_net_tool_context_tokens: usize,
    pub estimated_tokens_saved: usize,
    pub exposed_tool_count: usize,
    pub hidden_tool_count: usize,
    pub loaded_deferred_tool_count: usize,
}

#[derive(Debug, Clone)]
pub struct DeferredToolPrompt {
    pub exposed: Vec<ToolDefinition>,
    pub core: Vec<String>,
    pub loaded_deferred: Vec<String>,
    pub deferred: Vec<DeferredTool>,
    pub reminder: Option<String>,
    pub metrics: ToolSearchMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchResult {
    pub loaded: Vec<String>,
    pub already_loaded: Vec<String>,
    pub already_available: Vec<String>,
    pub not_found: Vec<String>,
    /// Rendered `<function>{...}</function>` definitions for every matched
    /// tool (loaded, already loaded, or already available).
    ///
    /// Provider gateways may defer tools behind their own search mechanism
    /// even when the tool is present in the request's tools array; their
    /// contract only makes a tool callable once its schema has appeared in
    /// the conversation. Always attaching the definition satisfies both the
    /// plain case and that gateway case, and matches the ToolSearch spec
    /// ("returns the matched tools' complete schema definitions").
    pub definitions: Vec<String>,
}

const SUBAGENT_PROHIBITED_AGENT_COORDINATION_TOOLS: &[&str] = &[
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
];

impl ToolSearchResult {
    pub fn is_error(&self) -> bool {
        self.loaded.is_empty()
            && self.already_loaded.is_empty()
            && self.already_available.is_empty()
            && !self.not_found.is_empty()
    }

    pub fn summary(&self) -> String {
        if self.is_error() {
            return format!(
                "Not found: {}. Only request exact tool names from the available tools list.",
                self.not_found.join(", ")
            );
        }

        let mut lines = Vec::new();
        if !self.loaded.is_empty() {
            lines.push(format!(
                "Loaded {} tool(s): {}",
                self.loaded.len(),
                self.loaded.join(", ")
            ));
        }
        if !self.already_loaded.is_empty() {
            lines.push(format!(
                "Already loaded {} tool(s): {}. Call these tools directly.",
                self.already_loaded.len(),
                self.already_loaded.join(", ")
            ));
        }
        if !self.already_available.is_empty() {
            lines.push(format!(
                "Already available {} tool(s): {}. Call these tools directly without loading.",
                self.already_available.len(),
                self.already_available.join(", ")
            ));
        }
        if !self.not_found.is_empty() {
            lines.push(format!(
                "Not found: {}. Only request exact tool names from the available tools list.",
                self.not_found.join(", ")
            ));
        }
        if !self.definitions.is_empty() {
            lines.push(String::new());
            lines.push(
                "Full definitions — every tool below is immediately callable exactly as defined:"
                    .to_string(),
            );
            lines.extend(self.definitions.iter().cloned());
        }
        lines.join("\n")
    }
}

pub fn hide_subagent_agent_coordination_tools(config: &mut DeferredLoadingConfig) {
    for name in [
        "spawn_agent",
        "send_message",
        "await_task",
        "list_tasks",
        "cancel_task",
        "wait_agent",
        "list_agents",
        "close_agent",
    ] {
        config.hidden.insert(name.to_string());
    }
}

pub fn is_subagent_agent_coordination_tool(name: &str) -> bool {
    SUBAGENT_PROHIBITED_AGENT_COORDINATION_TOOLS
        .iter()
        .any(|tool| name.eq_ignore_ascii_case(tool))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedDeferredTools {
    by_session: HashMap<String, BTreeSet<String>>,
}

/// ToolSearch is a simple lookup tool: `select:` loads exact names, and any
/// other query ranks registered tools by name+description tokens.
impl LoadedDeferredTools {
    pub fn mark_loaded(&mut self, session_id: &str, tool_name: &str) {
        self.by_session
            .entry(session_id.to_string())
            .or_default()
            .insert(tool_name.to_string());
    }

    pub fn is_loaded(&self, session_id: &str, tool_name: &str) -> bool {
        self.by_session
            .get(session_id)
            .is_some_and(|tools| tools.contains(tool_name))
    }

    pub fn list_loaded(&self, session_id: &str) -> BTreeSet<String> {
        self.by_session.get(session_id).cloned().unwrap_or_default()
    }
}

pub fn assemble_deferred_tool_prompt(
    all_tools: &[ToolDefinition],
    loaded: &BTreeSet<String>,
    config: &DeferredLoadingConfig,
) -> DeferredToolPrompt {
    if !config.enabled {
        let exposed = annotate_last_tool(all_tools.to_vec());
        let exposed_tokens = estimate_tool_tokens(&exposed);
        return DeferredToolPrompt {
            exposed,
            core: all_tools.iter().map(|tool| tool.name.clone()).collect(),
            loaded_deferred: Vec::new(),
            deferred: Vec::new(),
            reminder: None,
            metrics: ToolSearchMetrics {
                baseline_tool_schema_tokens: exposed_tokens,
                exposed_tool_schema_tokens: exposed_tokens,
                deferred_reminder_tokens: 0,
                estimated_net_tool_context_tokens: exposed_tokens,
                estimated_tokens_saved: 0,
                exposed_tool_count: all_tools.len(),
                hidden_tool_count: 0,
                loaded_deferred_tool_count: 0,
            },
        };
    }

    let mut exposed = Vec::new();
    let mut core = Vec::new();
    let mut loaded_deferred = Vec::new();
    let mut deferred = Vec::new();

    for tool in all_tools {
        match resolve_tool_policy(&tool.name, config) {
            PromptLoadingPolicy::Preloaded => {
                core.push(tool.name.clone());
                exposed.push(tool.clone());
            }
            PromptLoadingPolicy::Deferred => {
                if loaded.contains(&tool.name) {
                    loaded_deferred.push(tool.name.clone());
                    exposed.push(tool.clone());
                } else {
                    deferred.push(DeferredTool {
                        name: tool.name.clone(),
                        description: first_line(&tool.description),
                    });
                }
            }
            PromptLoadingPolicy::Hidden => {}
        }
    }

    let reminder = build_deferred_reminder(&deferred);
    let exposed = annotate_last_tool(exposed);
    let baseline_tool_schema_tokens = estimate_tool_tokens(all_tools);
    let exposed_tool_schema_tokens = estimate_tool_tokens(&exposed);
    let deferred_reminder_tokens = reminder.as_deref().map(estimate_text_tokens).unwrap_or(0);

    let hidden_tool_count = deferred.len();
    let loaded_deferred_tool_count = loaded_deferred.len();

    DeferredToolPrompt {
        exposed,
        core,
        loaded_deferred,
        deferred,
        reminder,
        metrics: ToolSearchMetrics {
            baseline_tool_schema_tokens,
            exposed_tool_schema_tokens,
            deferred_reminder_tokens,
            estimated_net_tool_context_tokens: exposed_tool_schema_tokens
                + deferred_reminder_tokens,
            estimated_tokens_saved: baseline_tool_schema_tokens
                .saturating_sub(exposed_tool_schema_tokens),
            exposed_tool_count: all_tools
                .iter()
                .filter(|tool| {
                    let policy = resolve_tool_policy(&tool.name, config);
                    policy == PromptLoadingPolicy::Preloaded
                        || (policy == PromptLoadingPolicy::Deferred && loaded.contains(&tool.name))
                })
                .count(),
            hidden_tool_count,
            loaded_deferred_tool_count,
        },
    }
}

pub fn execute_tool_search(
    session_id: &str,
    query: &str,
    all_tools: &[ToolDefinition],
    loaded: &mut LoadedDeferredTools,
    config: &DeferredLoadingConfig,
) -> Result<ToolSearchResult, String> {
    let query = query.trim();
    if let Some(selection) = query
        .strip_prefix("select:")
        .or_else(|| query.strip_prefix("SELECT:"))
    {
        return execute_select_tool_search(selection, session_id, all_tools, loaded, config);
    }

    execute_keyword_tool_search(query, all_tools, config)
}

fn execute_select_tool_search(
    selection: &str,
    session_id: &str,
    all_tools: &[ToolDefinition],
    loaded: &mut LoadedDeferredTools,
    config: &DeferredLoadingConfig,
) -> Result<ToolSearchResult, String> {
    let names = selection
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty());
    finish_tool_search(names, session_id, all_tools, Some(loaded), config)
}

fn execute_keyword_tool_search(
    query: &str,
    all_tools: &[ToolDefinition],
    config: &DeferredLoadingConfig,
) -> Result<ToolSearchResult, String> {
    if query.is_empty() {
        return Err("Expected a non-empty search query".to_string());
    }

    let query_tokens = keyword_tokens(query);
    if query_tokens.is_empty() {
        return Err("Expected a non-empty search query".to_string());
    }

    let mut scored = all_tools
        .iter()
        .filter_map(|tool| {
            let score = keyword_score(&query_tokens, tool);
            (score > 0).then_some((score, tool.name.as_str()))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    finish_tool_search(
        scored.into_iter().map(|(_, name)| name),
        /*session_id*/ "",
        all_tools,
        /*loaded_state*/ None,
        config,
    )
}

fn finish_tool_search<'a>(
    names: impl Iterator<Item = &'a str>,
    session_id: &str,
    all_tools: &[ToolDefinition],
    mut loaded_state: Option<&mut LoadedDeferredTools>,
    config: &DeferredLoadingConfig,
) -> Result<ToolSearchResult, String> {
    let registered: HashSet<_> = all_tools.iter().map(|tool| tool.name.as_str()).collect();
    let alias_map = alias_map(&registered);
    let mut result = ToolSearchResult {
        loaded: Vec::new(),
        already_loaded: Vec::new(),
        already_available: Vec::new(),
        not_found: Vec::new(),
        definitions: Vec::new(),
    };

    for requested_name in names {
        let canonical_name = resolve_alias(requested_name, &alias_map);
        let Some(tool) = all_tools.iter().find(|tool| tool.name == canonical_name) else {
            result.not_found.push(requested_name.to_string());
            continue;
        };

        match resolve_tool_policy(&canonical_name, config) {
            PromptLoadingPolicy::Preloaded => {
                if !result.already_available.contains(&canonical_name) {
                    result.already_available.push(canonical_name.clone());
                }
            }
            PromptLoadingPolicy::Deferred => match loaded_state.as_deref_mut() {
                // select: semantics — load the deferred tool for this session
                // so it counts as exposed from now on.
                Some(state) if state.is_loaded(session_id, &canonical_name) => {
                    if !result.already_loaded.contains(&canonical_name) {
                        result.already_loaded.push(canonical_name.clone());
                    }
                }
                Some(state) => {
                    state.mark_loaded(session_id, &canonical_name);
                    result.loaded.push(canonical_name.clone());
                }
                // Keyword discovery only surfaces names; loading stays
                // opt-in via select:.
                None => {
                    if !result.already_available.contains(&canonical_name) {
                        result.already_available.push(canonical_name.clone());
                    }
                }
            },
            PromptLoadingPolicy::Hidden => {
                result.not_found.push(requested_name.to_string());
                continue;
            }
        }
        result
            .definitions
            .push(function_definition_json(&canonical_name, tool));
    }

    if result.is_error() {
        return Err(result.summary());
    }

    Ok(result)
}

/// Render a tool as a gateway-portable `<function>{...}</function>`
/// definition (name/description/parameters), the shape deferred-tool
/// gateways look for before allowing a call.
fn function_definition_json(name: &str, tool: &ToolDefinition) -> String {
    let definition = serde_json::json!({
        "name": name,
        "description": tool.description,
        "parameters": tool.input_schema,
    });
    format!("<function>{definition}</function>")
}

fn keyword_tokens(query: &str) -> Vec<String> {
    let tokens = tokenize(query);
    if tokens.len() <= 1 {
        tokens
    } else {
        tokens
            .into_iter()
            .filter(|token| token.len() >= 2)
            .collect()
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn keyword_score(query_tokens: &[String], tool: &ToolDefinition) -> usize {
    let name_lower = tool.name.to_ascii_lowercase();
    let name_tokens = tokenize(&tool.name);
    let description_lower = tool.description.to_ascii_lowercase();
    let description_tokens = tokenize(&tool.description);
    let mut score = 0usize;
    for token in query_tokens {
        if name_lower == *token || name_tokens.iter().any(|name| name == token) {
            score += 3;
        } else if name_lower.contains(token) || name_tokens.iter().any(|name| name.contains(token))
        {
            score += 2;
        } else if description_tokens.iter().any(|part| part == token)
            || description_lower.contains(token)
        {
            score += 1;
        }
    }
    score
}

pub fn resolve_spec_policy(spec: &ToolSpec, config: &DeferredLoadingConfig) -> PromptLoadingPolicy {
    resolve_tool_policy(&spec.name, config)
}

pub fn resolve_tool_policy(name: &str, config: &DeferredLoadingConfig) -> PromptLoadingPolicy {
    if config.hidden.contains(name) {
        return PromptLoadingPolicy::Hidden;
    }
    if config.preloaded.contains(name) {
        return PromptLoadingPolicy::Preloaded;
    }
    if config.deferred.contains(name) {
        return PromptLoadingPolicy::Deferred;
    }
    if name.starts_with("mcp__") {
        return PromptLoadingPolicy::Deferred;
    }
    config.default_policy
}

fn build_deferred_reminder(deferred: &[DeferredTool]) -> Option<String> {
    if deferred.is_empty() {
        return None;
    }

    let mut reminder = String::from(
        "<system-reminder>\nThe following deferred tools are available via ToolSearch. Use query \"select:<name>[,<name>...]\" to load them.\nDeferred tools:",
    );
    for tool in deferred {
        reminder.push_str(&format!("\n  {}: {}", tool.name, tool.description));
    }
    reminder.push_str("\n</system-reminder>");
    Some(reminder)
}

fn annotate_last_tool(mut tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    if let Some(tool) = tools.last_mut()
        && !tool
            .description
            .contains("To load additional tools, use the ToolSearch tool.")
    {
        tool.description
            .push_str("\n\nTo load additional tools, use the ToolSearch tool.");
    }
    tools
}

fn alias_map(registered: &HashSet<&str>) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for name in registered {
        aliases.insert(name.to_lowercase(), (*name).to_string());
        aliases.insert(name.replace('_', "-").to_lowercase(), (*name).to_string());
        aliases.insert(
            name.replace(['_', '-'], "").to_lowercase(),
            (*name).to_string(),
        );
    }

    insert_alias(&mut aliases, registered, "bash", "shell_command");
    insert_alias(&mut aliases, registered, "shell", "shell_command");
    insert_alias(&mut aliases, registered, "terminal", "shell_command");
    insert_alias(&mut aliases, registered, "runcommand", "shell_command");
    insert_alias(&mut aliases, registered, "exec", "shell_command");
    insert_alias(&mut aliases, registered, "readfile", "read");
    insert_alias(&mut aliases, registered, "read-file", "read");
    insert_alias(&mut aliases, registered, "cat", "read");
    insert_alias(&mut aliases, registered, "view", "read");
    insert_alias(&mut aliases, registered, "glob", "find");
    insert_alias(&mut aliases, registered, "file-search", "find");
    insert_alias(&mut aliases, registered, "file_search", "find");
    insert_alias(&mut aliases, registered, "rg", "grep");
    insert_alias(&mut aliases, registered, "ripgrep", "grep");
    insert_alias(&mut aliases, registered, "fetch-url", "fetch_url");
    insert_alias(&mut aliases, registered, "urlfetch", "fetch_url");
    insert_alias(&mut aliases, registered, "websearch", "web_search");
    insert_alias(&mut aliases, registered, "web-search", "web_search");
    insert_alias(&mut aliases, registered, "fetch_url", "webfetch");
    insert_alias(&mut aliases, registered, "fetch-url", "webfetch");
    insert_alias(&mut aliases, registered, "tool-search", "ToolSearch");
    insert_alias(&mut aliases, registered, "tool_search", "ToolSearch");
    insert_alias(&mut aliases, registered, "loadtool", "ToolSearch");
    insert_alias(&mut aliases, registered, "subagent", "spawn_agent");
    insert_alias(&mut aliases, registered, "delegate", "spawn_agent");
    insert_alias(&mut aliases, registered, "spawn_subagent", "spawn_agent");
    insert_alias(&mut aliases, registered, "sub_agent", "spawn_agent");
    insert_alias(&mut aliases, registered, "subagent_status", "list_agents");
    insert_alias(&mut aliases, registered, "subagent_result", "wait_agent");
    aliases
}

fn insert_alias(
    aliases: &mut HashMap<String, String>,
    registered: &HashSet<&str>,
    alias: &str,
    canonical: &str,
) {
    if registered.contains(canonical) {
        aliases.insert(alias.to_string(), canonical.to_string());
    }
}

fn resolve_alias(requested_name: &str, alias_map: &HashMap<String, String>) -> String {
    alias_map
        .get(&requested_name.to_lowercase())
        .cloned()
        .unwrap_or_else(|| requested_name.to_string())
}

fn default_preloaded_tools() -> BTreeSet<String> {
    [
        "read",
        "find",
        "grep",
        "ls",
        "write",
        "apply_patch",
        "shell_command",
        "exec_command",
        "plan",
        "update_plan",
        "approval",
        "ToolSearch",
        "web_search",
        "webfetch",
        "web_fetch",
        "fetch_url",
        "spawn_agent",
        "send_message",
        "await_task",
        "list_tasks",
        "cancel_task",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_deferred_tools() -> BTreeSet<String> {
    ["multi_tool_use", "goal_update"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn first_line(description: &str) -> String {
    description.lines().next().unwrap_or_default().to_string()
}

fn estimate_tool_tokens(tools: &[ToolDefinition]) -> usize {
    estimate_text_tokens(&serde_json::to_string(tools).unwrap_or_default())
}

fn estimate_text_tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
        }
    }

    /// Struct-equality helper: reuse the produced definitions for the
    /// expected initializer, then assert their shape separately.
    fn result_definitions_snapshot(result: &ToolSearchResult) -> Vec<String> {
        result.definitions.clone()
    }

    fn tools() -> Vec<ToolDefinition> {
        vec![
            tool("read", "Read a file."),
            tool("find", "Find files."),
            tool("grep", "Search file contents."),
            tool("ToolSearch", "Search available tools."),
            tool(
                "web_search",
                "Search the web.\nLonger details are schema-only.",
            ),
            tool("fetch_url", "Fetch a URL."),
            tool("spawn_agent", "Spawn a subagent."),
            tool("secret_internal", "Internal lifecycle operation."),
        ]
    }

    fn agent_coordination_tools() -> Vec<ToolDefinition> {
        vec![
            tool("ToolSearch", "Search available tools."),
            tool("spawn_agent", "Spawn a subagent."),
            tool("send_message", "Send input to a child agent."),
            tool("await_task", "Wait for task completion."),
            tool("list_tasks", "List child tasks."),
            tool("cancel_task", "Cancel a child task."),
            tool("wait_agent", "Poll child output."),
            tool("list_agents", "List child agents."),
            tool("close_agent", "Close a child agent."),
        ]
    }

    #[test]
    fn classification_exposes_default_preloaded_tools() {
        let mut config = DeferredLoadingConfig::default();
        config.hidden.insert("secret_internal".to_string());
        let loaded = BTreeSet::from(["web_search".to_string()]);

        let prompt = assemble_deferred_tool_prompt(&tools(), &loaded, &config);

        assert_eq!(
            prompt
                .exposed
                .iter()
                .map(|tool| &tool.name)
                .collect::<Vec<_>>(),
            vec![
                "read",
                "find",
                "grep",
                "ToolSearch",
                "web_search",
                "fetch_url",
                "spawn_agent"
            ]
        );
        assert_eq!(
            prompt.core,
            vec![
                "read",
                "find",
                "grep",
                "ToolSearch",
                "web_search",
                "fetch_url",
                "spawn_agent"
            ]
        );
        assert_eq!(prompt.loaded_deferred, Vec::<String>::new());
        assert_eq!(prompt.deferred, Vec::<DeferredTool>::new());
        assert_eq!(prompt.reminder, None);
    }

    #[test]
    fn default_reminder_omits_preloaded_web_and_agent_tools() {
        let mut config = DeferredLoadingConfig::default();
        config.hidden.insert("secret_internal".to_string());

        let prompt = assemble_deferred_tool_prompt(&tools(), &BTreeSet::new(), &config);

        assert_eq!(prompt.reminder, None);
        assert_eq!(prompt.deferred, Vec::<DeferredTool>::new());
    }

    #[test]
    fn tool_search_resolves_aliases_and_reports_available_tools() {
        let config = DeferredLoadingConfig::default();
        let mut loaded = LoadedDeferredTools::default();

        let result = execute_tool_search(
            "session-1",
            "select:WebSearch,fetch-url,read,glob",
            &tools(),
            &mut loaded,
            &config,
        )
        .expect("tool search should return available tools");

        assert_eq!(
            result,
            ToolSearchResult {
                loaded: Vec::new(),
                already_loaded: Vec::new(),
                already_available: vec![
                    "web_search".to_string(),
                    "fetch_url".to_string(),
                    "read".to_string(),
                    "find".to_string(),
                ],
                not_found: Vec::new(),
                definitions: result_definitions_snapshot(&result),
            }
        );
        assert_eq!(result.definitions.len(), 4);
        assert!(!loaded.is_loaded("session-1", "web_search"));
        assert!(!loaded.is_loaded("session-1", "fetch_url"));
    }

    #[test]
    fn tool_search_keyword_ranks_name_and_description_tokens() {
        let config = DeferredLoadingConfig::default();
        let mut loaded = LoadedDeferredTools::default();

        let result = execute_tool_search("session-1", "read file", &tools(), &mut loaded, &config)
            .expect("keyword search should return matching tools");

        assert!(
            result.already_available.contains(&"read".to_string()),
            "expected read in {:?}",
            result.already_available
        );
        assert!(result.loaded.is_empty());
        assert!(result.already_loaded.is_empty());
        assert!(!loaded.is_loaded("session-1", "read"));
    }

    #[test]
    fn tool_search_resolves_agent_spawn_aliases() {
        let config = DeferredLoadingConfig::default();

        for requested in ["spawn_agent", "subagent", "delegate"] {
            let mut loaded = LoadedDeferredTools::default();
            let result = execute_tool_search(
                "session-1",
                &format!("select:{requested}"),
                &tools(),
                &mut loaded,
                &config,
            )
            .expect("agent spawn alias should resolve");

            assert_eq!(
                result,
                ToolSearchResult {
                    loaded: Vec::new(),
                    already_loaded: Vec::new(),
                    already_available: vec!["spawn_agent".to_string()],
                    not_found: Vec::new(),
                    definitions: result_definitions_snapshot(&result),
                }
            );
            assert_eq!(result.definitions.len(), 1);
            assert!(!loaded.is_loaded("session-1", "spawn_agent"));
        }
    }

    #[test]
    fn subagent_hidden_tools_hide_parent_agent_coordination_tools() {
        let mut config = DeferredLoadingConfig::default();
        hide_subagent_agent_coordination_tools(&mut config);

        for requested in [
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
            let mut loaded = LoadedDeferredTools::default();
            let err = execute_tool_search(
                "session-1",
                &format!("select:{requested}"),
                &agent_coordination_tools(),
                &mut loaded,
                &config,
            )
            .expect_err("subagent agent coordination tools should remain hidden");

            assert!(err.contains("Not found"));
            assert!(!loaded.is_loaded("session-1", "spawn_agent"));
            assert!(!loaded.is_loaded("session-1", "send_message"));
            assert!(!loaded.is_loaded("session-1", "await_task"));
            assert!(!loaded.is_loaded("session-1", "list_tasks"));
            assert!(!loaded.is_loaded("session-1", "cancel_task"));
            assert!(!loaded.is_loaded("session-1", "wait_agent"));
            assert!(!loaded.is_loaded("session-1", "list_agents"));
            assert!(!loaded.is_loaded("session-1", "close_agent"));
        }
    }

    #[test]
    fn subagent_hidden_tools_override_loaded_agent_coordination_tools() {
        let mut config = DeferredLoadingConfig::default();
        hide_subagent_agent_coordination_tools(&mut config);
        let loaded = BTreeSet::from([
            "spawn_agent".to_string(),
            "send_message".to_string(),
            "await_task".to_string(),
            "list_tasks".to_string(),
            "cancel_task".to_string(),
            "wait_agent".to_string(),
            "list_agents".to_string(),
            "close_agent".to_string(),
        ]);

        let prompt = assemble_deferred_tool_prompt(&agent_coordination_tools(), &loaded, &config);

        assert_eq!(
            prompt
                .exposed
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ToolSearch"]
        );
        assert_eq!(prompt.loaded_deferred, Vec::<String>::new());
        assert_eq!(prompt.deferred, Vec::<DeferredTool>::new());
        assert_eq!(prompt.reminder, None);
    }

    #[test]
    fn tool_search_resolves_bash_alias_to_shell_command() {
        let config = DeferredLoadingConfig::default();
        let mut loaded = LoadedDeferredTools::default();
        let tools = vec![
            tool("shell_command", "Run a shell command."),
            tool("ToolSearch", "Search available tools."),
        ];

        let result = execute_tool_search("session-1", "select:bash", &tools, &mut loaded, &config)
            .expect("bash alias should resolve");

        assert_eq!(
            result,
            ToolSearchResult {
                loaded: Vec::new(),
                already_loaded: Vec::new(),
                already_available: vec!["shell_command".to_string()],
                not_found: Vec::new(),
                definitions: vec![format!(
                    "<function>{}</function>",
                    json!({
                        "name": "shell_command",
                        "description": "Run a shell command.",
                        "parameters": {"type": "object"},
                    })
                )],
            }
        );
    }

    #[test]
    fn mcp_tools_are_deferred_by_default() {
        let config = DeferredLoadingConfig {
            default_policy: PromptLoadingPolicy::Preloaded,
            ..DeferredLoadingConfig::default()
        };

        assert_eq!(
            resolve_tool_policy("mcp__docs__search", &config),
            PromptLoadingPolicy::Deferred
        );
    }

    #[test]
    fn tool_search_reports_available_and_not_found() {
        let config = DeferredLoadingConfig::default();
        let mut loaded = LoadedDeferredTools::default();
        loaded.mark_loaded("session-1", "web_search");

        let result = execute_tool_search(
            "session-1",
            "select:web_search,missing",
            &tools(),
            &mut loaded,
            &config,
        )
        .expect("partial success should return summary");

        assert_eq!(
            result,
            ToolSearchResult {
                loaded: Vec::new(),
                already_loaded: Vec::new(),
                already_available: vec!["web_search".to_string()],
                not_found: vec!["missing".to_string()],
                definitions: vec![format!(
                    "<function>{}</function>",
                    json!({
                        "name": "web_search",
                        "description": "Search the web.\nLonger details are schema-only.",
                        "parameters": {"type": "object"},
                    })
                )],
            }
        );
        assert!(
            result
                .summary()
                .contains("Already available 1 tool(s): web_search")
        );
        assert!(result.summary().contains("Not found: missing"));
        // The definition must accompany even "already available" tools —
        // provider gateways that defer tools only allow calls once the
        // schema has appeared in the conversation.
        assert!(result.summary().contains("<function>"));
        assert!(result.summary().contains("\"name\":\"web_search\""));
    }

    #[test]
    fn tool_search_errors_when_all_requested_tools_are_unknown() {
        let mut loaded = LoadedDeferredTools::default();
        let err = execute_tool_search(
            "session-1",
            "select:imaginary",
            &tools(),
            &mut loaded,
            &DeferredLoadingConfig::default(),
        )
        .expect_err("all unknown tools should error");

        assert_eq!(
            err,
            "Not found: imaginary. Only request exact tool names from the available tools list."
        );
    }

    #[test]
    fn tool_search_does_not_resolve_removed_code_search_alias() {
        let mut loaded = LoadedDeferredTools::default();
        let tools = vec![
            tool("read", "Read a file."),
            tool("ToolSearch", "Search available tools."),
        ];

        let err = execute_tool_search(
            "session-1",
            "select:semble",
            &tools,
            &mut loaded,
            &DeferredLoadingConfig::default(),
        )
        .expect_err("removed code_search alias should not resolve");

        assert_eq!(
            err,
            "Not found: semble. Only request exact tool names from the available tools list."
        );
    }

    #[test]
    fn loaded_tools_are_session_scoped_and_listed_deterministically() {
        let mut loaded = LoadedDeferredTools::default();
        loaded.mark_loaded("s1", "skill");
        loaded.mark_loaded("s1", "web_search");
        loaded.mark_loaded("s2", "fetch_url");

        assert_eq!(
            loaded.list_loaded("s1"),
            BTreeSet::from(["skill".to_string(), "web_search".to_string()])
        );
        assert_eq!(
            loaded.list_loaded("s2"),
            BTreeSet::from(["fetch_url".to_string()])
        );
    }

    #[test]
    fn select_loads_deferred_tool_and_returns_schema_definition() {
        // Regression: ToolSearch used to answer "Already available … call
        // these tools directly without loading" for deferred tools without
        // ever marking them loaded and without attaching a schema. Models
        // behind deferred-tool gateways (which only allow calls after the
        // schema appears in-conversation) looped on the same select: query
        // dozens of times per turn.
        let config = DeferredLoadingConfig::default();
        let tools = vec![
            tool("ipython", "Execute Python code in the session RLM kernel."),
            tool("ToolSearch", "Search available tools."),
        ];
        let mut loaded = LoadedDeferredTools::default();

        let first = execute_tool_search("s1", "select:ipython", &tools, &mut loaded, &config)
            .expect("select should succeed");
        assert_eq!(first.loaded, vec!["ipython".to_string()]);
        assert!(first.already_available.is_empty());
        assert!(loaded.is_loaded("s1", "ipython"));
        let summary = first.summary();
        assert!(summary.contains("Loaded 1 tool(s): ipython"));
        assert!(summary.contains("<function>"));
        assert!(summary.contains("\"name\":\"ipython\""));
        assert!(summary.contains("\"parameters\""));

        // Re-selecting degrades to already-loaded but still carries the
        // definition, so the tool stays callable in the eyes of a gateway.
        let second = execute_tool_search("s1", "select:ipython", &tools, &mut loaded, &config)
            .expect("second select should succeed");
        assert_eq!(second.already_loaded, vec!["ipython".to_string()]);
        assert!(second.definitions.len() == 1);
        assert!(
            second
                .summary()
                .contains("Already loaded 1 tool(s): ipython")
        );
        assert!(second.summary().contains("<function>"));

        // Loading stays session-scoped.
        let other = execute_tool_search("s2", "select:ipython", &tools, &mut loaded, &config)
            .expect("other session select should succeed");
        assert_eq!(other.loaded, vec!["ipython".to_string()]);
    }

    #[test]
    fn keyword_search_reports_definition_without_loading() {
        let config = DeferredLoadingConfig::default();
        let tools = vec![
            tool("ipython", "Execute Python code in the session RLM kernel."),
            tool("ToolSearch", "Search available tools."),
        ];
        let mut loaded = LoadedDeferredTools::default();

        let result = execute_tool_search("s1", "python repl", &tools, &mut loaded, &config)
            .expect("keyword search should succeed");
        assert!(result.loaded.is_empty());
        assert!(!loaded.is_loaded("s1", "ipython"));
        assert!(result.definitions.len() == 1);
        assert!(result.summary().contains("<function>"));
        assert!(result.summary().contains("\"name\":\"ipython\""));
    }

    #[test]
    fn prompt_metrics_report_default_preloaded_tools() {
        let mut config = DeferredLoadingConfig::default();
        config.hidden.insert("secret_internal".to_string());
        let prompt = assemble_deferred_tool_prompt(&tools(), &BTreeSet::new(), &config);

        assert_eq!(prompt.metrics.exposed_tool_count, 7);
        assert_eq!(prompt.metrics.hidden_tool_count, 0);
        assert_eq!(prompt.metrics.loaded_deferred_tool_count, 0);
        assert_eq!(prompt.metrics.deferred_reminder_tokens, 0);
        assert_eq!(prompt.metrics.estimated_tokens_saved, 0);
        assert_eq!(
            prompt.metrics.estimated_net_tool_context_tokens,
            prompt.metrics.exposed_tool_schema_tokens
        );
    }
}
