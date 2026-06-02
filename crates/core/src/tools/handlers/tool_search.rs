use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use bm25::Document;
use bm25::Language;
use bm25::SearchEngine;
use bm25::SearchEngineBuilder;
use devo_protocol::ToolDefinition;

use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::deferred_loading::DeferredLoadingConfig;
use crate::deferred_loading::LoadedDeferredTools;
use crate::deferred_loading::PromptLoadingPolicy;
use crate::deferred_loading::execute_tool_search;
use crate::deferred_loading::resolve_tool_policy;
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolSpec;

const TOOL_SEARCH_DEFAULT_LIMIT: usize = 8;

#[derive(Clone)]
pub struct ToolSearchEntry {
    definition: ToolDefinition,
    search_text: String,
}

pub struct ToolSearchHandler {
    definitions: Vec<ToolDefinition>,
    entries: Vec<ToolSearchEntry>,
    loaded_tools: Arc<Mutex<LoadedDeferredTools>>,
    config: DeferredLoadingConfig,
    search_engine: SearchEngine<usize>,
    spec: ToolSpec,
}

impl ToolSearchHandler {
    pub fn new(
        definitions: Vec<(ToolDefinition, Option<String>)>,
        loaded_tools: Arc<Mutex<LoadedDeferredTools>>,
        config: DeferredLoadingConfig,
    ) -> Self {
        let all_definitions = definitions
            .iter()
            .map(|(definition, _)| definition.clone())
            .collect::<Vec<_>>();
        let entries = definitions
            .into_iter()
            .filter(|(definition, _)| {
                resolve_tool_policy(&definition.name, &config) == PromptLoadingPolicy::Deferred
            })
            .map(|(definition, search_text)| ToolSearchEntry {
                search_text: search_text.unwrap_or_else(|| default_search_text(&definition)),
                definition,
            })
            .collect::<Vec<_>>();
        let documents = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| Document::new(idx, entry.search_text.clone()))
            .collect::<Vec<_>>();
        let search_engine =
            SearchEngineBuilder::<usize>::with_documents(Language::English, documents).build();

        Self {
            definitions: all_definitions,
            entries,
            loaded_tools,
            config,
            search_engine,
            spec: tool_search_spec(),
        }
    }
}

#[async_trait]
impl ToolHandler for ToolSearchHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let query = input["query"].as_str().ok_or_else(|| {
            ToolCallError::InvalidInput("Expected non-empty string field `query`".into())
        })?;
        let query = query.trim();
        if query.is_empty() {
            return Err(ToolCallError::InvalidInput(
                "Expected non-empty string field `query`".into(),
            ));
        }
        let limit = input["limit"]
            .as_u64()
            .map(|limit| limit as usize)
            .unwrap_or(TOOL_SEARCH_DEFAULT_LIMIT);
        if limit == 0 {
            return Err(ToolCallError::InvalidInput(
                "Expected `limit` to be greater than zero".into(),
            ));
        }

        let selection = if is_select_query(query) {
            query.to_string()
        } else {
            let names = self.search(query, limit);
            if names.is_empty() {
                return Ok(ToolResult::success(
                    ToolResultContent::Text("No matching deferred tools found.".to_string()),
                    "No tools loaded",
                ));
            }
            format!("select:{}", names.join(","))
        };

        let mut loaded_tools = self.loaded_tools.lock().map_err(|_| {
            ToolCallError::InternalError("loaded deferred tool state lock poisoned".into())
        })?;
        let result = execute_tool_search(
            &ctx.session_id,
            &selection,
            &self.definitions,
            &mut loaded_tools,
            &self.config,
        )
        .map_err(ToolCallError::ExecutionFailed)?;

        Ok(ToolResult::success(
            ToolResultContent::Text(result.summary()),
            "Tools loaded",
        ))
    }
}

impl ToolSearchHandler {
    fn search(&self, query: &str, limit: usize) -> Vec<String> {
        self.search_engine
            .search(query, limit)
            .into_iter()
            .filter_map(|result| self.entries.get(result.document.id))
            .map(|entry| entry.definition.name.clone())
            .collect()
    }
}

pub fn tool_search_spec() -> ToolSpec {
    ToolSpec {
        name: "ToolSearch".to_string(),
        description: format!(
            "Searches deferred tool metadata with BM25 and loads matching tool schemas for the next model request. Use natural-language queries, or `select:<name>[,<name>...]` for exact compatibility. Defaults to {TOOL_SEARCH_DEFAULT_LIMIT} results."
        ),
        input_schema: JsonSchema::object(
            std::collections::BTreeMap::from([
                (
                    "query".to_string(),
                    JsonSchema::string(Some("Search query for deferred tools.")),
                ),
                (
                    "limit".to_string(),
                    JsonSchema::number(Some("Maximum number of tools to load.")),
                ),
            ]),
            Some(vec!["query".to_string()]),
            Some(false),
        ),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: true,
        preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
        display_name: Some("ToolSearch".to_string()),
        supports_cancellation: None,
        supports_streaming: None,
    }
}

fn is_select_query(query: &str) -> bool {
    query.starts_with("select:") || query.starts_with("SELECT:")
}

fn default_search_text(definition: &ToolDefinition) -> String {
    let mut schema_properties = definition
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    schema_properties.sort();
    let mut parts = vec![definition.name.clone(), definition.description.clone()];
    parts.extend(schema_properties);
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn definition(name: &str, description: &str, schema: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: schema,
            output_schema: None,
        }
    }

    #[tokio::test]
    async fn natural_language_search_loads_matching_deferred_tool() {
        let loaded_tools = Arc::new(Mutex::new(LoadedDeferredTools::default()));
        let handler = ToolSearchHandler::new(
            vec![
                (
                    definition("read", "Read files", serde_json::json!({"type": "object"})),
                    None,
                ),
                (
                    definition(
                        "mcp__docs__search",
                        "Search docs",
                        serde_json::json!({
                            "type": "object",
                            "properties": { "query": { "type": "string" } }
                        }),
                    ),
                    Some("mcp__docs__search docs knowledge base query".to_string()),
                ),
            ],
            Arc::clone(&loaded_tools),
            DeferredLoadingConfig::default(),
        );

        let result = handler
            .handle(
                ToolContext {
                    tool_call_id: crate::invocation::ToolCallId("call".to_string()),
                    session_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    workspace_root: std::path::PathBuf::from("."),
                    budgets: crate::contracts::ToolBudgets {
                        output_limit_bytes: 1024,
                        wall_time_limit_ms: None,
                    },
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                },
                serde_json::json!({ "query": "knowledge base" }),
                None,
            )
            .await
            .expect("tool search should succeed");

        assert_eq!(result.result_summary, "Tools loaded");
        let loaded_tools = loaded_tools.lock().expect("loaded tools");
        assert!(loaded_tools.is_loaded("session-1", "mcp__docs__search"));
    }
}
