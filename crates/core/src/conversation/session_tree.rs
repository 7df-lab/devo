//! In-session transcript tree: leaf pointer, parent edges, path-from-leaf.
//!
//! Builds nested tree nodes for `session/tree/read` and selects the
//! effective model-context path as ancestry from the current leaf to root.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use devo_protocol::native::ids::ItemId;
use devo_protocol::native::item::{Item, ItemEnvelope, UserInput};
use devo_protocol::native::rpc_session::SessionTreeNode;
use serde_json::{Value as JsonValue, json};

use super::history::CanonicalHistory;

/// Whether an item appears in the InteractiveMode session tree.
pub fn is_tree_visible_item(item: &Item) -> bool {
    matches!(
        item,
        Item::UserMessage { .. }
            | Item::AssistantMessage { .. }
            | Item::Reasoning { .. }
            | Item::BranchSummary { .. }
            | Item::ToolCall { .. }
            | Item::ToolResult { .. }
            | Item::CommandExecution { .. }
            | Item::ContextCompaction { .. }
    )
}

/// Resolves parent pointers: TreeEdge and item `parent_id` win; otherwise
/// infer a linear chain by ascending `seq` among tree-visible items.
pub fn resolve_parent_map(history: &CanonicalHistory) -> HashMap<ItemId, Option<ItemId>> {
    let mut parents: HashMap<ItemId, Option<ItemId>> = history.tree_edges.clone();
    let mut explicit: HashSet<ItemId> = history.tree_edges.keys().copied().collect();
    for envelope in &history.items {
        if let Some(parent_id) = envelope.parent_id {
            parents.insert(envelope.id, Some(parent_id));
            explicit.insert(envelope.id);
        }
    }
    let mut visible: Vec<&ItemEnvelope> = history
        .items
        .iter()
        .filter(|envelope| is_tree_visible_item(&envelope.item))
        .collect();
    visible.sort_by_key(|envelope| envelope.seq);

    let mut prev: Option<ItemId> = None;
    for envelope in &visible {
        // Only infer when no durable edge exists (including explicit root None).
        parents.entry(envelope.id).or_insert(prev);
        prev = Some(envelope.id);
    }

    // Stitch pass — repair journals whose compaction summaries were written
    // without a parent edge (pre-fix binaries): an un-edged run of compaction
    // rows whose inferred anchor equals the explicit parent claimed by the
    // NEXT edged item belongs ON that chain, not beside it. Without this, the
    // post-compaction item's explicit edge skips over the summary row and
    // drops it off the active path. The compaction-only bound keeps legacy
    // no-edge prefixes and rewinds untouched: dead-branch rows are never
    // stitched onto the path.
    let mut run: Vec<ItemId> = Vec::new();
    let mut run_all_compaction = true;
    let mut run_anchor: Option<ItemId> = None;
    for envelope in &visible {
        let id = envelope.id;
        if explicit.contains(&id) {
            if !run.is_empty()
                && run_all_compaction
                && run_anchor.is_some()
                && parents.get(&id).cloned().flatten() == run_anchor
                && let Some(last) = run.last()
            {
                parents.insert(id, Some(*last));
            }
            run.clear();
            run_all_compaction = true;
            run_anchor = None;
        } else {
            if run.is_empty() {
                run_anchor = parents.get(&id).cloned().flatten();
                run_all_compaction = matches!(envelope.item, Item::ContextCompaction { .. });
            } else {
                run_all_compaction &= matches!(envelope.item, Item::ContextCompaction { .. });
            }
            run.push(id);
        }
    }
    parents
}

/// Effective leaf: durable SessionLeaf when set and present; else last
/// tree-visible item by seq.
pub fn resolve_leaf_id(
    history: &CanonicalHistory,
    parents: &HashMap<ItemId, Option<ItemId>>,
) -> Option<ItemId> {
    if let Some(leaf) = history.leaf_id
        && (parents.contains_key(&leaf) || history.items.iter().any(|item| item.id == leaf))
    {
        return Some(leaf);
    }
    history
        .items
        .iter()
        .filter(|envelope| is_tree_visible_item(&envelope.item))
        .max_by_key(|envelope| envelope.seq)
        .map(|envelope| envelope.id)
}

/// Walk parent links from `leaf` to root (root first).
pub fn path_root_to_leaf(
    leaf: Option<&ItemId>,
    parents: &HashMap<ItemId, Option<ItemId>>,
) -> Vec<ItemId> {
    let Some(start) = leaf else {
        return Vec::new();
    };
    let mut chain = Vec::new();
    let mut current = Some(*start);
    let mut seen = HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            break;
        }
        chain.push(id);
        current = parents.get(&id).cloned().flatten();
    }
    chain.reverse();
    chain
}

/// Item ids on the active root→leaf path (for model context filtering).
pub fn active_path_item_ids(history: &CanonicalHistory) -> HashSet<ItemId> {
    let parents = resolve_parent_map(history);
    let leaf = resolve_leaf_id(history, &parents);
    path_root_to_leaf(leaf.as_ref(), &parents)
        .into_iter()
        .collect()
}

/// Active-path view of the journal's item envelopes: the "current
/// conversation" a fresh client should render. Tree-visible rows are kept
/// only when they sit on the active root→leaf path; rows that never render
/// in the tree (SubAgent, Warning, …) follow their turn. Returns `None`
/// when no path resolves (no or empty tree — single-line sessions and
/// forks), leaving callers the linear history.
pub fn active_path_items(history: &CanonicalHistory) -> Option<Vec<ItemEnvelope>> {
    let active = active_path_item_ids(history);
    if active.is_empty() {
        return None;
    }
    let active_turn_ids: HashSet<_> = history
        .items
        .iter()
        .filter(|envelope| active.contains(&envelope.id))
        .map(|envelope| envelope.turn_id)
        .collect();
    Some(
        history
            .items
            .iter()
            .filter(|envelope| {
                active.contains(&envelope.id)
                    || (!is_tree_visible_item(&envelope.item)
                        && active_turn_ids.contains(&envelope.turn_id))
            })
            .cloned()
            .collect(),
    )
}

/// Build the nested tree + resolved leaf id.
pub fn build_session_tree(history: &CanonicalHistory) -> (Vec<SessionTreeNode>, Option<ItemId>) {
    let parents = resolve_parent_map(history);
    let leaf_id = resolve_leaf_id(history, &parents);

    let mut order: Vec<ItemId> = Vec::new();
    let mut by_id: HashMap<ItemId, &ItemEnvelope> = HashMap::new();
    for envelope in history
        .items
        .iter()
        .filter(|e| is_tree_visible_item(&e.item))
    {
        order.push(envelope.id);
        by_id.insert(envelope.id, envelope);
    }
    let visible_ids: HashSet<ItemId> = order.iter().copied().collect();

    // Attach children (stable by original seq order). A node's direct
    // parent may not itself be a tree node — it can be a non-visible row
    // (e.g. a Plan) or an item dropped by a rollback whose child kept its
    // explicit edge. Thread past such ancestors to the nearest VISIBLE one;
    // when the ancestor chain dead-ends on an item absent from history (or
    // cycles), keep the timeline connected by attaching to the closest
    // earlier visible node — for a rollback cut that is exactly the
    // retained cut point. Without this, the conversation fragments into a
    // new disconnected root at every gap.
    let mut roots: Vec<ItemId> = Vec::new();
    let mut child_ids: HashMap<ItemId, Vec<ItemId>> = HashMap::new();
    let nearest_visible_ancestor = |id: &ItemId| -> Option<ItemId> {
        let mut cursor = parents.get(id).cloned().flatten()?;
        for _ in 0..=parents.len() {
            if visible_ids.contains(&cursor) {
                return Some(cursor);
            }
            cursor = parents.get(&cursor).cloned().flatten()?;
        }
        None
    };
    for (index, id) in order.iter().enumerate() {
        let parent = nearest_visible_ancestor(id).or_else(|| {
            order[..index]
                .iter()
                .rev()
                .find(|candidate| visible_ids.contains(candidate))
                .copied()
        });
        match parent {
            Some(parent) => child_ids.entry(parent).or_default().push(*id),
            None => roots.push(*id),
        }
    }

    // Nodes carry the ATTACHMENT parent — the visible node they actually
    // hang under — never the raw resolved edge, which can name an invisible
    // row (Plan, injected system input) or a rollback-dropped id. Clients
    // that walk `entry.parentId` from the leaf (fork picker, HTML export)
    // would truncate the active path at every such seam.
    fn assemble(
        id: &ItemId,
        attachment_parent: Option<&ItemId>,
        by_id: &HashMap<ItemId, &ItemEnvelope>,
        child_ids: &HashMap<ItemId, Vec<ItemId>>,
    ) -> Option<SessionTreeNode> {
        let envelope = *by_id.get(id)?;
        let mut node = SessionTreeNode {
            entry: project_tree_entry(envelope, attachment_parent),
            label: tree_label(envelope),
            label_timestamp: None,
            children: Vec::new(),
        };
        if let Some(children) = child_ids.get(id) {
            for child in children {
                if let Some(child_node) = assemble(child, Some(id), by_id, child_ids) {
                    node.children.push(child_node);
                }
            }
        }
        Some(node)
    }

    let tree = roots
        .iter()
        .filter_map(|id| assemble(id, None, &by_id, &child_ids))
        .collect();
    (tree, leaf_id)
}

fn tree_label(envelope: &ItemEnvelope) -> Option<String> {
    match &envelope.item {
        Item::UserMessage { content, .. } => Some(user_text(content).chars().take(80).collect()),
        Item::AssistantMessage { text, .. } => Some(text.chars().take(80).collect()),
        Item::Reasoning { text, .. } => Some(format!(
            "thinking: {}",
            text.chars().take(60).collect::<String>()
        )),
        Item::BranchSummary { summary, .. } => Some(summary.chars().take(80).collect()),
        Item::ToolCall { tool_name, .. } => Some(format!("tool: {tool_name}")),
        Item::ToolResult { call_id, .. } => Some(format!("result: {call_id}")),
        Item::CommandExecution { command, .. } => Some(format!(
            "$ {}",
            command.chars().take(70).collect::<String>()
        )),
        Item::ContextCompaction { summary, .. } => Some(format!(
            "compacted: {}",
            compacted_summary_text(summary.as_deref())
                .chars()
                .take(70)
                .collect::<String>()
        )),
        _ => None,
    }
}

/// First content line of a compaction summary, without the model-facing
/// `<compaction_summary>` XML wrapper the summarizer emits.
fn compacted_summary_text(summary: Option<&str>) -> &str {
    let text = summary.unwrap_or("");
    let stripped = text
        .strip_prefix("<compaction_summary>")
        .unwrap_or(text)
        .trim();
    stripped
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

fn user_text(content: &[UserInput]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            UserInput::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn project_tree_entry(envelope: &ItemEnvelope, parent_id: Option<&ItemId>) -> JsonValue {
    let timestamp = format_timestamp(envelope.created_at);
    let parent = parent_id
        .map(|id| json!(id.as_str()))
        .unwrap_or(JsonValue::Null);
    match &envelope.item {
        Item::UserMessage { content, .. } => json!({
            "type": "message",
            "id": envelope.id.as_str(),
            "parentId": parent,
            "timestamp": timestamp,
            "message": {
                "role": "user",
                "content": content.iter().filter_map(|part| match part {
                    UserInput::Text { text } => Some(json!({ "type": "text", "text": text })),
                    _ => None,
                }).collect::<Vec<_>>(),
            }
        }),
        Item::AssistantMessage { text, .. } => json!({
            "type": "message",
            "id": envelope.id.as_str(),
            "parentId": parent,
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": text }],
            }
        }),
        Item::Reasoning { text, .. } => json!({
            "type": "message",
            "id": envelope.id.as_str(),
            "parentId": parent,
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "content": [{ "type": "thinking", "thinking": text }],
            }
        }),
        Item::BranchSummary { summary, details } => json!({
            "type": "branch_summary",
            "id": envelope.id.as_str(),
            "parentId": parent,
            "timestamp": timestamp,
            "summary": summary,
            "details": details,
        }),
        Item::ToolCall {
            call_id, tool_name, ..
        } => json!({
            "type": "message",
            "id": envelope.id.as_str(),
            "parentId": parent,
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "toolCall",
                    "id": call_id,
                    "name": tool_name,
                    "arguments": {},
                }],
            }
        }),
        Item::ToolResult { call_id, .. } => json!({
            "type": "message",
            "id": envelope.id.as_str(),
            "parentId": parent,
            "timestamp": timestamp,
            "message": {
                "role": "toolResult",
                "toolCallId": call_id,
                "toolName": "tool",
                "content": [{ "type": "text", "text": "" }],
            }
        }),
        Item::CommandExecution { command, .. } => json!({
            "type": "message",
            "id": envelope.id.as_str(),
            "parentId": parent,
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "bash",
                    "command": command,
                }],
            }
        }),
        Item::ContextCompaction {
            summary, before, ..
        } => json!({
            "type": "compaction",
            "id": envelope.id.as_str(),
            "parentId": parent,
            "timestamp": timestamp,
            "summary": summary,
            "tokensBefore": before.total_tokens,
        }),
        _ => json!({
            "type": "custom",
            "id": envelope.id.as_str(),
            "parentId": parent,
            "timestamp": timestamp,
            "customType": "item",
        }),
    }
}

fn format_timestamp(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod active_path_items_tests {
    use super::*;
    use devo_protocol::native::item::{
        Item, ItemEnvelope, ItemState, SpawnedWorkState, UserInput, UserMessageEntry,
    };
    use pretty_assertions::assert_eq;

    fn user_message(text: &str) -> Item {
        Item::UserMessage {
            client_user_message_id: None,
            content: vec![UserInput::Text {
                text: text.to_owned(),
            }],
            entry: UserMessageEntry::TurnStart,
        }
    }

    fn assistant_message(text: &str) -> Item {
        Item::AssistantMessage {
            text: text.to_owned(),
        }
    }

    fn sub_agent(task: &str) -> Item {
        Item::SubAgent {
            origin_call_id: None,
            agent_session_id: devo_protocol::SessionId::new(),
            parent_session_id: devo_protocol::SessionId::new(),
            role: None,
            task: task.to_owned(),
            state: SpawnedWorkState::Completed,
        }
    }

    fn envelope(id: ItemId, seq: u64, parent_id: Option<ItemId>, item: Item) -> ItemEnvelope {
        ItemEnvelope {
            id,
            session_id: devo_protocol::SessionId::new(),
            turn_id: devo_protocol::TurnId::new(),
            seq,
            revision: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            state: ItemState::Completed,
            item,
            parent_id,
        }
    }

    /// Linear journal of a rewound session: turn 1 kept, turn 2 abandoned,
    /// turn 3 the new branch. A SubAgent row rides the dead turn to prove
    /// non-tree-visible rows follow their turn off the path.
    fn rewound_history(item_ids: [ItemId; 7]) -> CanonicalHistory {
        CanonicalHistory {
            items: vec![
                envelope(item_ids[0], 1, None, user_message("first user")),
                envelope(
                    item_ids[1],
                    2,
                    Some(item_ids[0]),
                    assistant_message("first answer"),
                ),
                envelope(
                    item_ids[2],
                    3,
                    Some(item_ids[1]),
                    user_message("dead branch user"),
                ),
                envelope(
                    item_ids[3],
                    4,
                    Some(item_ids[2]),
                    sub_agent("dead branch agent"),
                ),
                envelope(
                    item_ids[4],
                    5,
                    Some(item_ids[2]),
                    assistant_message("dead branch answer"),
                ),
                envelope(
                    item_ids[5],
                    6,
                    Some(item_ids[1]),
                    user_message("rewound user"),
                ),
                envelope(
                    item_ids[6],
                    7,
                    Some(item_ids[5]),
                    assistant_message("rewound answer"),
                ),
            ],
            tree_edges: HashMap::new(),
            leaf_id: Some(item_ids[6]),
            ..CanonicalHistory::default()
        }
    }

    fn texts(items: &[ItemEnvelope]) -> Vec<String> {
        items
            .iter()
            .map(|envelope| match &envelope.item {
                Item::UserMessage { content, .. } => content
                    .iter()
                    .map(|input| match input {
                        UserInput::Text { text } => text.clone(),
                        other => panic!("unexpected user input: {other:?}"),
                    })
                    .collect::<String>(),
                Item::AssistantMessage { text } => text.clone(),
                Item::ContextCompaction { summary, .. } => summary.clone().unwrap_or_default(),
                Item::SubAgent { task, .. } => task.clone(),
                other => panic!("unexpected item: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn tree_threads_through_invisible_and_dropped_parents() {
        // Regression (live r25): an update_plan turn writes an Item::Plan
        // mid-chain; the plan row is not tree-visible, so the next item's
        // explicit edge to it left the ENTIRE remainder of the conversation
        // as a second disconnected root. The same fragmentation hit the
        // first post-rollback write, whose edge pointed into a dropped turn.
        let u1 = ItemId::new();
        let plan = ItemId::new();
        let a1 = ItemId::new();
        let u2 = ItemId::new();
        let dropped = ItemId::new(); // rollback-dropped: absent from history
        let history = CanonicalHistory {
            items: vec![
                envelope(u1, 1, None, user_message("first user")),
                envelope(
                    plan,
                    2,
                    Some(u1),
                    Item::Plan {
                        call_id: None,
                        entries: Vec::new(),
                    },
                ),
                envelope(a1, 3, Some(plan), assistant_message("answer after plan")),
                envelope(u2, 4, Some(dropped), user_message("post-rollback user")),
            ],
            tree_edges: HashMap::new(),
            leaf_id: Some(u2),
            ..CanonicalHistory::default()
        };
        let (tree, leaf_id) = build_session_tree(&history);
        assert_eq!(tree.len(), 1, "one connected tree, not fragment roots");
        assert_eq!(tree[0].entry["id"].as_str(), Some(u1.as_str()));
        let a1_node = &tree[0]
            .children
            .iter()
            .find(|child| child.entry["id"].as_str() == Some(a1.as_str()))
            .expect("post-plan item threads past the invisible plan to its visible ancestor");
        assert!(
            a1_node
                .children
                .iter()
                .any(|child| child.entry["id"].as_str() == Some(u2.as_str())),
            "dead-end edge adopts the closest earlier visible node"
        );
        assert_eq!(leaf_id, Some(u2));
    }

    #[test]
    fn active_path_items_drops_dead_branch_and_its_unlisted_rows() {
        let item_ids: Vec<ItemId> = (0..7).map(|_| ItemId::new()).collect();
        let history = rewound_history(item_ids.clone().try_into().unwrap());

        let scoped = active_path_items(&history).expect("active path resolves");

        assert_eq!(
            texts(&scoped),
            vec![
                "first user",
                "first answer",
                "rewound user",
                "rewound answer"
            ]
        );
    }

    #[test]
    fn active_path_items_covers_treeless_linear_history_and_empty_is_none() {
        // No tree records: readers infer a linear parent chain, so the whole
        // session is the active path (same rows as the linear fallback).
        let item_ids: Vec<ItemId> = (0..2).map(|_| ItemId::new()).collect();
        let linear = CanonicalHistory {
            items: vec![
                envelope(item_ids[0], 1, None, user_message("only user")),
                envelope(
                    item_ids[1],
                    2,
                    Some(item_ids[0]),
                    assistant_message("only answer"),
                ),
            ],
            ..CanonicalHistory::default()
        };
        let scoped = active_path_items(&linear).expect("inferred linear path");
        assert_eq!(texts(&scoped), vec!["only user", "only answer"]);

        assert_eq!(active_path_items(&CanonicalHistory::default()), None);
    }

    fn context_compaction(summary: &str) -> Item {
        Item::ContextCompaction {
            trigger: devo_protocol::native::item::CompactionTrigger::AutoThreshold,
            before: devo_protocol::native::item::ContextUsage::default(),
            after: None,
            summary: Some(summary.to_owned()),
        }
    }

    /// Regression: compaction summaries written before the tree-chaining fix
    /// carried no parent edge, so the post-compaction item's explicit edge
    /// skipped over them and dropped the summary rows off the active path.
    #[test]
    fn active_path_items_stitches_un_edged_compaction_summaries_into_the_chain() {
        let item_ids: Vec<ItemId> = (0..8).map(|_| ItemId::new()).collect();
        let history = CanonicalHistory {
            items: vec![
                envelope(item_ids[0], 1, None, user_message("u1")),
                envelope(item_ids[1], 2, Some(item_ids[0]), assistant_message("a1")),
                envelope(item_ids[2], 3, Some(item_ids[1]), user_message("u2")),
                // Pre-fix compaction: no parent edge, no tree edge.
                envelope(item_ids[3], 4, None, context_compaction("sum1")),
                // Explicit edge skips the compaction row (old writer bug).
                envelope(item_ids[4], 5, Some(item_ids[2]), assistant_message("a2")),
                envelope(item_ids[5], 6, Some(item_ids[4]), user_message("u3")),
                envelope(item_ids[6], 7, None, context_compaction("sum2")),
                envelope(item_ids[7], 8, Some(item_ids[5]), assistant_message("a3")),
            ],
            tree_edges: HashMap::new(),
            leaf_id: Some(item_ids[7]),
            ..CanonicalHistory::default()
        };

        let scoped = active_path_items(&history).expect("active path resolves");

        // All 8 rows stay on the path, in journal order.
        assert_eq!(
            texts(&scoped),
            vec!["u1", "a1", "u2", "sum1", "a2", "u3", "sum2", "a3"]
        );
    }

    /// The stitch must not pull a dead branch back onto the path: a legacy
    /// no-edge prefix rewound by an explicit edge stays rewound.
    #[test]
    fn active_path_items_does_not_stitch_legacy_rows_past_a_rewind() {
        let item_ids: Vec<ItemId> = (0..5).map(|_| ItemId::new()).collect();
        let history = CanonicalHistory {
            items: vec![
                // Legacy migrated prefix: no parent edges anywhere.
                envelope(item_ids[0], 1, None, user_message("first user")),
                envelope(item_ids[1], 2, None, user_message("dead user")),
                envelope(item_ids[2], 3, None, assistant_message("dead answer")),
                // Rewind to the first message: explicit edge past the dead run.
                envelope(item_ids[3], 4, Some(item_ids[0]), user_message("new user")),
                envelope(
                    item_ids[4],
                    5,
                    Some(item_ids[3]),
                    assistant_message("new answer"),
                ),
            ],
            tree_edges: HashMap::new(),
            leaf_id: Some(item_ids[4]),
            ..CanonicalHistory::default()
        };

        let scoped = active_path_items(&history).expect("active path resolves");

        assert_eq!(texts(&scoped), vec!["first user", "new user", "new answer"]);
    }

    /// Regression (live r25 `/fork`): entries emitted their RAW resolved
    /// parent, which can name an invisible row (injected system input, a
    /// Plan) or a rollback-dropped id. Walking `entry.parentId` from the
    /// leaf then stopped at every seam, so the fork picker offered only
    /// messages newer than the last mid-turn injection. Entries must carry
    /// the visible node they hang under.
    #[test]
    fn tree_entry_parent_ids_reference_visible_tree_nodes() {
        let u1 = ItemId::new();
        let injected = ItemId::new(); // invisible system row mid-chain
        let a1 = ItemId::new();
        let u2 = ItemId::new();
        let dropped = ItemId::new(); // rollback-dropped: absent from history
        let a2 = ItemId::new();
        let history = CanonicalHistory {
            items: vec![
                envelope(u1, 1, None, user_message("first user")),
                envelope(
                    injected,
                    2,
                    Some(u1),
                    Item::Plan {
                        call_id: None,
                        entries: Vec::new(),
                    },
                ),
                envelope(
                    a1,
                    3,
                    Some(injected),
                    assistant_message("answer after plan"),
                ),
                envelope(u2, 4, Some(dropped), user_message("post-rollback user")),
                envelope(a2, 5, Some(u2), assistant_message("final answer")),
            ],
            tree_edges: HashMap::new(),
            leaf_id: Some(a2),
            ..CanonicalHistory::default()
        };
        let (tree, leaf_id) = build_session_tree(&history);
        assert_eq!(leaf_id, Some(a2));
        assert_eq!(tree.len(), 1, "one connected tree, not fragment roots");

        let mut parent_by_id: HashMap<String, Option<String>> = HashMap::new();
        fn collect(nodes: &[SessionTreeNode], parent_by_id: &mut HashMap<String, Option<String>>) {
            for node in nodes {
                parent_by_id.insert(
                    node.entry["id"].as_str().expect("id").to_string(),
                    node.entry["parentId"].as_str().map(str::to_string),
                );
                collect(&node.children, parent_by_id);
            }
        }
        collect(&tree, &mut parent_by_id);

        // Walk entry.parentId from leaf to root — every hop must land on a
        // tree node and the chain must reach the root.
        let mut chain: Vec<String> = Vec::new();
        let mut cursor = a2.as_str().to_string();
        while let Some(parent) = parent_by_id[&cursor].clone() {
            assert!(
                parent_by_id.contains_key(&parent),
                "entry.parentId must name a tree node, got {parent}"
            );
            chain.push(parent.clone());
            cursor = parent;
        }
        assert_eq!(
            chain,
            vec![u2.as_str(), a1.as_str(), u1.as_str()],
            "leaf-to-root chain threads past the invisible plan and the dropped edge"
        );
    }

    /// The tree label must not leak the model-facing `<compaction_summary>`
    /// XML wrapper into the UI.
    #[test]
    fn session_tree_labels_compaction_without_xml_wrapper() {
        let item = Item::ContextCompaction {
            trigger: devo_protocol::native::item::CompactionTrigger::AutoThreshold,
            before: devo_protocol::native::item::ContextUsage::default(),
            after: None,
            summary: Some(
                "<compaction_summary>\nAnother lengthy exchange about QUEUE messages.\n</compaction_summary>"
                    .to_string(),
            ),
        };
        let env = envelope(ItemId::new(), 1, None, item);

        assert_eq!(
            tree_label(&env),
            Some("compacted: Another lengthy exchange about QUEUE messages.".to_string())
        );
    }
}
