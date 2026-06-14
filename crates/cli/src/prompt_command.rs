use anyhow::Result;
use clap::ValueEnum;
use devo_core::AgentsMdConfig;
use devo_core::AppConfig;
use devo_core::AppConfigLoader;
use devo_core::EventCallback;
use devo_core::FileSystemAppConfigLoader;
use devo_core::ModelCatalog;
use devo_core::PresetModelCatalog;
use devo_core::QueryEvent;
use devo_core::tools::ToolPlanConfig;
use devo_core::tools::handlers;
use devo_mcp::manager::RmcpMcpManager;
use devo_util_paths::find_devo_home;
use serde::Serialize;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum PromptOutputFormat {
    Text,
    Json,
    Jsonl,
}

pub(crate) async fn run_prompt(
    input: &str,
    model_override: Option<&str>,
    log_level: Option<&str>,
    output_format: PromptOutputFormat,
) -> Result<()> {
    if let Some(level) = log_level {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(level))
            .try_init();
    }
    use devo_core::SessionConfig;
    use devo_core::SessionState;
    use devo_core::default_base_instructions;
    use devo_core::tools::ToolRuntime;

    let cwd = std::env::current_dir()?;
    let home_dir = find_devo_home()?;
    let app_config = FileSystemAppConfigLoader::new(home_dir.clone())
        .load(Some(cwd.as_path()))
        .unwrap_or_else(|_| AppConfig::default());
    let provider = devo_server::load_server_provider(&app_config, model_override, &home_dir)?;
    let selected_model = model_override
        .map(ToString::to_string)
        .unwrap_or_else(|| provider.default_model.clone());

    let mut session_state = SessionState::new(
        SessionConfig {
            agents_md: AgentsMdConfig {
                project_root_markers: app_config.project_root_markers.clone(),
                ..AgentsMdConfig::default()
            },
            ..SessionConfig::default()
        },
        cwd.clone(),
    );
    session_state.push_message(devo_core::Message::user(input.to_string()));

    let registry = {
        let mcp_manager = std::sync::Arc::new(RmcpMcpManager::new(
            app_config.mcp.clone(),
            app_config.mcp_oauth_credentials_store.unwrap_or_default(),
        ));
        let tool_plan = ToolPlanConfig::from_app_config(&app_config);
        let reg = handlers::build_registry_from_plan_with_mcp(&tool_plan, mcp_manager).await;
        std::sync::Arc::new(reg)
    };
    let runtime = ToolRuntime::new_with_context(
        std::sync::Arc::clone(&registry),
        devo_core::tools::PermissionChecker::always_allow(),
        devo_core::tools::ToolRuntimeContext {
            session_id: session_state.id.clone(),
            turn_id: None,
            cwd: cwd.clone(),
            agent_scope: devo_core::tools::ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator: None,
            local_web_search: None,
            hooks: (!app_config.hooks.is_empty()).then(|| devo_core::HookRuntimeContext {
                runner: devo_core::HookRunner::new(app_config.hooks.clone()),
                base: devo_core::HookBaseInput {
                    session_id: session_state.id.clone(),
                    transcript_path: String::new(),
                    cwd: cwd.clone(),
                    permission_mode: Some("auto-approve".to_string()),
                    agent_id: None,
                    agent_type: None,
                },
            }),
            network_proxy: None,
        },
    );
    let model_catalog = PresetModelCatalog::load_from_config(&home_dir, Some(&cwd))?;

    let turn_config = devo_core::TurnConfig::new(
        model_catalog
            .get(&selected_model)
            .cloned()
            .unwrap_or_else(|| devo_core::Model {
                slug: selected_model.clone(),
                base_instructions: default_base_instructions().to_string(),
                ..Default::default()
            }),
        /*thinking_selection*/ None,
    );

    eprintln!("devo [prompt] model={selected_model} sending...");

    if output_format == PromptOutputFormat::Jsonl {
        write_jsonl(&PromptJsonlEvent::SessionStarted {
            session_id: session_state.id.as_str(),
            model: selected_model.as_str(),
            cwd: cwd.as_path(),
        })?;
        write_jsonl(&PromptJsonlEvent::TurnStarted {
            session_id: session_state.id.as_str(),
            model: selected_model.as_str(),
        })?;
    }

    let session_id_for_events = session_state.id.clone();
    let result = devo_core::query(
        &mut session_state,
        &turn_config,
        provider.provider.clone(),
        registry,
        &runtime,
        jsonl_event_callback(output_format, session_id_for_events),
    )
    .await;

    match result {
        Ok(()) => match latest_assistant_text(&session_state.messages) {
            Some(text) => match output_format {
                PromptOutputFormat::Text => println!("{}", text),
                PromptOutputFormat::Json => write_json(&PromptResult {
                    r#type: "result",
                    status: "completed",
                    session_id: session_state.id.as_str(),
                    model: selected_model.as_str(),
                    message: text,
                    usage: PromptUsage::from_session(&session_state),
                })?,
                PromptOutputFormat::Jsonl => write_jsonl(&PromptJsonlEvent::Result {
                    session_id: session_state.id.as_str(),
                    status: "completed",
                    message: text,
                    usage: PromptUsage::from_session(&session_state),
                })?,
            },
            None => eprintln!("devo [prompt] empty response"),
        },
        Err(e) => {
            if output_format == PromptOutputFormat::Jsonl {
                write_jsonl(&PromptJsonlEvent::Error {
                    session_id: session_state.id.as_str(),
                    message: &e.to_string(),
                })?;
                write_jsonl(&PromptJsonlEvent::TurnFailed {
                    session_id: session_state.id.as_str(),
                    message: &e.to_string(),
                })?;
            }
            anyhow::bail!("prompt failed: {e}");
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PromptUsage {
    input_tokens: usize,
    output_tokens: usize,
    cache_creation_input_tokens: usize,
    cache_read_input_tokens: usize,
}

impl PromptUsage {
    fn from_session(session: &devo_core::SessionState) -> Self {
        Self {
            input_tokens: session.total_input_tokens,
            output_tokens: session.total_output_tokens,
            cache_creation_input_tokens: session.total_cache_creation_tokens,
            cache_read_input_tokens: session.total_cache_read_tokens,
        }
    }
}

#[derive(Debug, Serialize)]
struct PromptResult<'a> {
    r#type: &'static str,
    status: &'static str,
    session_id: &'a str,
    model: &'a str,
    message: &'a str,
    usage: PromptUsage,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PromptJsonlEvent<'a> {
    #[serde(rename = "session.started")]
    SessionStarted {
        session_id: &'a str,
        model: &'a str,
        cwd: &'a Path,
    },
    #[serde(rename = "turn.started")]
    TurnStarted { session_id: &'a str, model: &'a str },
    #[serde(rename = "item.updated")]
    AssistantMessageDelta {
        session_id: &'a str,
        item_type: &'static str,
        delta: &'a str,
    },
    #[serde(rename = "item.updated")]
    ReasoningDelta {
        session_id: &'a str,
        item_type: &'static str,
        delta: &'a str,
    },
    #[serde(rename = "item.completed")]
    ReasoningCompleted {
        session_id: &'a str,
        item_type: &'static str,
    },
    #[serde(rename = "item.started")]
    ToolCallStarted {
        session_id: &'a str,
        item_type: &'static str,
        tool_call_id: &'a str,
        tool_name: &'a str,
        input: &'a serde_json::Value,
    },
    #[serde(rename = "item.updated")]
    ToolProgress {
        session_id: &'a str,
        item_type: &'static str,
        tool_call_id: &'a str,
        delta: &'a str,
    },
    #[serde(rename = "item.completed")]
    ToolResult {
        session_id: &'a str,
        item_type: &'static str,
        tool_call_id: &'a str,
        tool_name: &'a str,
        input: &'a serde_json::Value,
        content: &'a devo_core::tools::ToolContent,
        display_content: &'a Option<String>,
        is_error: bool,
        summary: &'a str,
    },
    #[serde(rename = "turn.usage_delta")]
    UsageDelta {
        session_id: &'a str,
        usage: PromptUsageDelta,
    },
    #[serde(rename = "turn.usage")]
    Usage {
        session_id: &'a str,
        usage: PromptUsageDelta,
    },
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        session_id: &'a str,
        stop_reason: &'a devo_core::StopReason,
    },
    #[serde(rename = "turn.failed")]
    TurnFailed {
        session_id: &'a str,
        message: &'a str,
    },
    #[serde(rename = "error")]
    Error {
        session_id: &'a str,
        message: &'a str,
    },
    #[serde(rename = "result")]
    Result {
        session_id: &'a str,
        status: &'static str,
        message: &'a str,
        usage: PromptUsage,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct PromptUsageDelta {
    input_tokens: usize,
    output_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_creation_input_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read_input_tokens: Option<usize>,
}

impl PromptUsageDelta {
    fn new(
        input_tokens: usize,
        output_tokens: usize,
        cache_creation_input_tokens: Option<usize>,
        cache_read_input_tokens: Option<usize>,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
        }
    }
}

fn jsonl_event_callback(
    output_format: PromptOutputFormat,
    session_id: String,
) -> Option<EventCallback> {
    if output_format != PromptOutputFormat::Jsonl {
        return None;
    }

    Some(Arc::new(move |event| {
        if let Err(error) = write_query_event_jsonl(session_id.as_str(), &event) {
            eprintln!("devo [prompt] failed to write jsonl event: {error}");
        }
    }))
}

fn write_query_event_jsonl(session_id: &str, event: &QueryEvent) -> Result<()> {
    match event {
        QueryEvent::TextDelta(text) => write_jsonl(&PromptJsonlEvent::AssistantMessageDelta {
            session_id,
            item_type: "agent_message",
            delta: text,
        }),
        QueryEvent::ReasoningDelta(text) => write_jsonl(&PromptJsonlEvent::ReasoningDelta {
            session_id,
            item_type: "reasoning",
            delta: text,
        }),
        QueryEvent::ReasoningCompleted => write_jsonl(&PromptJsonlEvent::ReasoningCompleted {
            session_id,
            item_type: "reasoning",
        }),
        QueryEvent::UsageDelta {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
        } => write_jsonl(&PromptJsonlEvent::UsageDelta {
            session_id,
            usage: PromptUsageDelta::new(
                *input_tokens,
                *output_tokens,
                *cache_creation_input_tokens,
                *cache_read_input_tokens,
            ),
        }),
        QueryEvent::ToolUseStart { id, name, input } => {
            write_jsonl(&PromptJsonlEvent::ToolCallStarted {
                session_id,
                item_type: "tool_call",
                tool_call_id: id,
                tool_name: name,
                input,
            })
        }
        QueryEvent::ToolProgress {
            tool_use_id,
            content,
        } => write_jsonl(&PromptJsonlEvent::ToolProgress {
            session_id,
            item_type: "tool_result",
            tool_call_id: tool_use_id,
            delta: content,
        }),
        QueryEvent::ToolResult {
            tool_use_id,
            tool_name,
            input,
            content,
            display_content,
            is_error,
            summary,
        } => write_jsonl(&PromptJsonlEvent::ToolResult {
            session_id,
            item_type: "tool_result",
            tool_call_id: tool_use_id,
            tool_name,
            input,
            content,
            display_content,
            is_error: *is_error,
            summary,
        }),
        QueryEvent::TurnComplete { stop_reason } => write_jsonl(&PromptJsonlEvent::TurnCompleted {
            session_id,
            stop_reason,
        }),
        QueryEvent::Usage {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
        } => write_jsonl(&PromptJsonlEvent::Usage {
            session_id,
            usage: PromptUsageDelta::new(
                *input_tokens,
                *output_tokens,
                *cache_creation_input_tokens,
                *cache_read_input_tokens,
            ),
        }),
    }
}

fn write_json<T: Serialize>(value: &T) -> Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}

fn write_jsonl<T: Serialize>(value: &T) -> Result<()> {
    write_json(value)
}

fn latest_assistant_text(messages: &[devo_core::Message]) -> Option<&str> {
    messages.iter().rev().find_map(|message| {
        if message.role != devo_core::Role::Assistant {
            return None;
        }
        message
            .content
            .iter()
            .find_map(|block| match block {
                devo_core::ContentBlock::Text { text } => Some(text.as_str()),
                devo_core::ContentBlock::Reasoning { .. }
                | devo_core::ContentBlock::ProviderReasoning { .. }
                | devo_core::ContentBlock::ToolUse { .. }
                | devo_core::ContentBlock::HostedToolUse { .. }
                | devo_core::ContentBlock::ToolResult { .. } => None,
            })
            .or_else(|| {
                message.content.iter().find_map(|block| match block {
                    devo_core::ContentBlock::Reasoning { text } => Some(text.as_str()),
                    devo_core::ContentBlock::Text { .. }
                    | devo_core::ContentBlock::ProviderReasoning { .. }
                    | devo_core::ContentBlock::ToolUse { .. }
                    | devo_core::ContentBlock::HostedToolUse { .. }
                    | devo_core::ContentBlock::ToolResult { .. } => None,
                })
            })
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::latest_assistant_text;
    use super::{PromptResult, PromptUsage};
    use devo_core::ContentBlock;
    use devo_core::Message;
    use devo_core::Role;

    #[test]
    fn prompt_result_serializes_completed_json_shape() {
        let value = serde_json::to_value(PromptResult {
            r#type: "result",
            status: "completed",
            session_id: "session-1",
            model: "model-1",
            message: "done",
            usage: PromptUsage {
                input_tokens: 3,
                output_tokens: 5,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 2,
            },
        })
        .expect("serialize prompt result");

        assert_eq!(
            value,
            serde_json::json!({
                "type": "result",
                "status": "completed",
                "session_id": "session-1",
                "model": "model-1",
                "message": "done",
                "usage": {
                    "input_tokens": 3,
                    "output_tokens": 5,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 2
                }
            })
        );
    }

    #[test]
    fn latest_assistant_text_returns_none_for_empty_messages() {
        assert_eq!(latest_assistant_text(&[]), None);
    }

    #[test]
    fn latest_assistant_text_ignores_user_messages() {
        assert_eq!(latest_assistant_text(&[Message::user("hello")]), None);
    }

    #[test]
    fn latest_assistant_text_returns_latest_assistant_text() {
        let messages = vec![
            Message::assistant_text("older"),
            Message::user("next"),
            Message::assistant_text("newer"),
        ];

        assert_eq!(latest_assistant_text(&messages), Some("newer"));
    }

    #[test]
    fn latest_assistant_text_skips_assistant_messages_without_text() {
        let messages = vec![
            Message::assistant_text("fallback"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tool-call".to_string(),
                    name: "tool".to_string(),
                    input: serde_json::json!({}),
                }],
            },
        ];

        assert_eq!(latest_assistant_text(&messages), Some("fallback"));
    }

    #[test]
    fn latest_assistant_text_uses_first_text_block_within_latest_assistant_message() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "tool-call".to_string(),
                    content: "ignored".to_string(),
                    is_error: false,
                },
                ContentBlock::Text {
                    text: "first text".to_string(),
                },
                ContentBlock::Text {
                    text: "second text".to_string(),
                },
            ],
        }];

        assert_eq!(latest_assistant_text(&messages), Some("first text"));
    }

    #[test]
    fn latest_assistant_text_prefers_text_over_reasoning() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning {
                    text: "internal summary".to_string(),
                },
                ContentBlock::Text {
                    text: "final answer".to_string(),
                },
            ],
        }];

        assert_eq!(latest_assistant_text(&messages), Some("final answer"));
    }
}
