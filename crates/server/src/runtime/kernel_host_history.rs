//! Model-facing, bounded reads of the canonical active transcript.
use std::borrow::Cow;

use devo_protocol::native::ids::SessionId;
use devo_protocol::native::item::{Item, ItemEnvelope, UserInput};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ServerRuntime;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    #[serde(default)]
    query: String,
    kind: Option<Kind>,
    call_id: Option<String>,
    before: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    item_id: String,
    #[serde(default)]
    offset: usize,
    max_chars: Option<usize>,
    #[serde(default)]
    around: usize,
}

enum Request {
    Search(Search),
    Read(Read),
}

pub(super) async fn recall(
    runtime: &ServerRuntime,
    session: SessionId,
    action: &str,
    params: Value,
) -> Result<Value, String> {
    let request = match action {
        "rlm.history.search" => serde_json::from_value(params).map(Request::Search),
        "rlm.history.read" => serde_json::from_value(params).map(Request::Read),
        _ => return Err("unknown history action".into()),
    }
    .map_err(|e| format!("invalid history request: {e}"))?;
    let history = runtime
        .load_canonical_history(&Value::Null, session)
        .await
        .map_err(|_| "recorded history unavailable for this session".to_owned())?;
    tokio::task::spawn_blocking(move || {
        let rows = recallable_rows(&history, session);
        match request {
            Request::Search(params) => search(&rows, params),
            Request::Read(params) => read(&rows, params),
        }
    })
    .await
    .map_err(|e| format!("history reader failed: {e}"))?
}

// On a branched transcript even normally UI-hidden records must be on the path.
// A shared turn ID alone cannot prove that a record survived a rewind.
fn recallable_rows(
    history: &devo_core::CanonicalHistory,
    session: SessionId,
) -> Vec<&ItemEnvelope> {
    let has_tree = history.leaf_epoch != 0
        || history.leaf_id.is_some()
        || !history.tree_edges.is_empty()
        || history.items.iter().any(|row| row.parent_id.is_some());
    let active = has_tree.then(|| devo_core::active_path_item_ids(history));
    history
        .items
        .iter()
        .filter(|row| {
            row.session_id == session
                && kind(&row.item).is_some()
                && active.as_ref().is_none_or(|ids| ids.contains(&row.id))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    User,
    Host,
    Assistant,
    ToolCall,
    ToolResult,
    Command,
    FileChange,
    HostedTool,
    Plan,
    Summary,
}

fn kind(item: &Item) -> Option<Kind> {
    Some(match item {
        Item::UserMessage { origin: None, .. } => Kind::User,
        Item::UserMessage {
            origin: Some(_), ..
        } => Kind::Host,
        Item::AssistantMessage { .. } => Kind::Assistant,
        Item::ToolCall { .. } => Kind::ToolCall,
        Item::ToolResult { .. } => Kind::ToolResult,
        Item::CommandExecution { .. } => Kind::Command,
        Item::FileChange { .. } => Kind::FileChange,
        Item::HostedToolCall { .. } => Kind::HostedTool,
        Item::Plan { .. } => Kind::Plan,
        Item::ContextCompaction {
            summary: Some(_), ..
        }
        | Item::BranchSummary { .. } => Kind::Summary,
        _ => return None,
    })
}

fn body(item: &Item) -> Cow<'_, str> {
    match item {
        Item::UserMessage { content, .. } => match content.as_slice() {
            [UserInput::Text { text }] => Cow::Borrowed(text),
            parts => Cow::Owned(
                parts
                    .iter()
                    .map(|part| match part {
                        UserInput::Text { text } => text.as_str(),
                        UserInput::Image { .. } | UserInput::LocalImage { .. } => "[image]",
                        UserInput::Audio { .. } => "[audio]",
                        UserInput::Skill { name } => name,
                        UserInput::Mention { uri } => uri,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        },
        Item::AssistantMessage { text } => Cow::Borrowed(text),
        Item::ToolCall {
            tool_name, input, ..
        } => Cow::Owned(format!("{tool_name}\n{}", json_text(input.as_ref()))),
        Item::ToolResult { output, .. } => json_text(Some(output)),
        Item::CommandExecution {
            command,
            input,
            output,
            ..
        } => Cow::Owned(format!(
            "{command}\n{}\n{}",
            json_text(input.as_ref()),
            json_text(output.as_ref())
        )),
        Item::HostedToolCall {
            tool_name,
            input,
            output,
            ..
        } => Cow::Owned(format!(
            "{tool_name}\n{}\n{}",
            json_text(input.as_ref()),
            json_text(output.as_ref())
        )),
        Item::FileChange { changes, .. } => {
            Cow::Owned(serde_json::to_string(changes).expect("file changes serialize"))
        }
        Item::Plan { entries, .. } => {
            Cow::Owned(serde_json::to_string(entries).expect("plan serializes"))
        }
        Item::BranchSummary { summary, .. }
        | Item::ContextCompaction {
            summary: Some(summary),
            ..
        } => Cow::Borrowed(summary),
        _ => unreachable!("reader only projects recallable items"),
    }
}

fn json_text(value: Option<&Value>) -> Cow<'_, str> {
    match value {
        None => Cow::Borrowed(""),
        Some(Value::String(text)) => Cow::Borrowed(text),
        Some(value) => Cow::Owned(value.to_string()),
    }
}

fn call_id(item: &Item) -> Option<&str> {
    match item {
        Item::FileChange { call_id, .. }
        | Item::Plan {
            call_id: Some(call_id),
            ..
        } => Some(call_id),
        _ => crate::persisted_native_item::tool_call_id(item),
    }
}

fn metadata(row: &ItemEnvelope) -> Value {
    let mut result = json!({"item_id":row.id, "turn_id":row.turn_id, "kind":kind(&row.item)});
    if let Some(call_id) = call_id(&row.item) {
        result["call_id"] = json!(call_id);
    }
    match &row.item {
        Item::UserMessage {
            origin: Some(origin),
            ..
        } => result["origin"] = json!(origin),
        Item::ToolResult {
            truncated,
            is_error,
            ..
        } => {
            result["source_truncated"] = json!(truncated);
            result["is_error"] = json!(is_error);
        }
        Item::CommandExecution {
            is_error,
            exit_code,
            ..
        } => {
            result["is_error"] = json!(is_error);
            result["exit_code"] = json!(exit_code);
        }
        _ => {}
    }
    result
}

fn preview(row: &ItemEnvelope, text: &str, offset: usize) -> Value {
    let start = offset.saturating_sub(48);
    let mut entry = metadata(row);
    entry["offset"] = json!(offset);
    entry["preview"] = json!(text.chars().skip(start).take(240).collect::<String>());
    entry
}

fn search(rows: &[&ItemEnvelope], params: Search) -> Result<Value, String> {
    let limit = params.limit.unwrap_or(6);
    if !(1..=20).contains(&limit) || params.query.chars().count() > 256 {
        return Err("limit must be 1..20 and query at most 256 characters".into());
    }
    let end = match params.before {
        Some(before) => rows
            .iter()
            .position(|row| row.id.as_str() == before)
            .ok_or("history cursor is no longer on the active branch")?,
        None => rows.len(),
    };
    let mut matches = Vec::new();
    let mut next_before = None;
    for &row in rows[..end].iter().rev() {
        if params.kind.is_some() && params.kind != kind(&row.item)
            || params.call_id.is_some() && params.call_id.as_deref() != call_id(&row.item)
        {
            continue;
        }
        let content = body(&row.item);
        if let Some(byte) = content.find(&params.query) {
            if matches.len() == limit {
                next_before = matches
                    .last()
                    .and_then(|entry: &Value| entry["item_id"].as_str());
                break;
            }
            matches.push(preview(row, &content, content[..byte].chars().count()));
        }
    }
    Ok(json!({"matches":matches,"next_before":next_before}))
}

fn read(rows: &[&ItemEnvelope], params: Read) -> Result<Value, String> {
    let limit = params.max_chars.unwrap_or(3000);
    if !(1..=8000).contains(&limit) || params.around > 2 {
        return Err("max_chars must be 1..8000 and around at most 2".into());
    }
    let index = if let Some(id) = params.item_id.strip_prefix("call:") {
        let mut matches = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| call_id(&r.item) == Some(id));
        let (mut index, first) = matches
            .next()
            .ok_or("tool call not found on active branch")?;
        for (i, row) in matches {
            if first.turn_id != row.turn_id
                || !matches!(rows[index].item, Item::ToolCall { .. })
                || matches!(row.item, Item::ToolCall { .. })
            {
                return Err(
                    "ambiguous call id; use history.search(call_id=...) and read the item_id"
                        .into(),
                );
            }
            index = i;
        }
        index
    } else {
        rows.iter()
            .position(|r| r.id.as_str() == params.item_id)
            .ok_or("item not found on active branch")?
    };
    let row = rows[index];
    let content = body(&row.item);
    let mut chars = content.chars();
    if params.offset > 0 && chars.nth(params.offset - 1).is_none() {
        return Err("offset beyond end of record".into());
    }
    let page: String = chars.by_ref().take(limit).collect();
    let mut result = metadata(row);
    result["offset"] = json!(params.offset);
    result["next_offset"] = json!(chars.next().map(|_| params.offset + limit));
    result["text"] = Value::String(page);
    if let Item::ToolResult { call_id, .. } = &row.item
        && let Some(call) = rows[..index].iter().rev().find(|r| {
            r.turn_id == row.turn_id
                && matches!(&r.item, Item::ToolCall { call_id: other, .. } if other == call_id)
        })
    {
        result["tool_call"] = preview(call, &body(&call.item), 0);
    }
    result["neighbors"] = json!(
        (index.saturating_sub(params.around)..(index + params.around + 1).min(rows.len()))
            .filter(|&i| i != index)
            .map(|i| preview(rows[i], &body(&rows[i].item), 0))
            .collect::<Vec<_>>()
    );
    Ok(result)
}

#[cfg(test)]
mod tests;
