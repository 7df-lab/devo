//! Handlers for the paged history read methods of the new Native API:
//! `session/turns/list` and `session/items/list` (01 §4.2, 07).
//!
//! The in-memory runtime model does not retain turn records or item
//! envelopes, so both methods read the session's rollout file through
//! `devo_core::read_canonical_history` (dual-format, fail-closed). That also
//! makes cold sessions work without a resume: resolving the rollout path is
//! enough.

use std::path::PathBuf;
use std::sync::Arc;

use devo_core::active_path_items;
use devo_protocol::native::item::ItemEnvelope;
use devo_protocol::native::page::{Page, PageParams};
use devo_protocol::native::rpc_session::{SessionItemsListParams, SessionTurnsListParams};

use super::super::*;

/// Default page size for the history read methods.
const DEFAULT_PAGE_LIMIT: u32 = 50;
/// Maximum page size; larger requests are clamped, not rejected (01 §4.2).
const MAX_PAGE_LIMIT: u32 = 200;

impl ServerRuntime {
    pub(crate) async fn handle_session_turns_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionTurnsListParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/turns/list params: {error}"),
                );
            }
        };
        let history = match self
            .load_canonical_history(&request_id, params.session_id)
            .await
        {
            Ok(history) => history,
            Err(response) => return response,
        };
        // Keep turn records consistent with the active-path item view above:
        // turns whose items all sit on an abandoned branch are dead history.
        let turns = match active_path_items(&history) {
            Some(items) => {
                let active_turn_ids: std::collections::HashSet<_> =
                    items.iter().map(|item| item.turn_id).collect();
                history
                    .turns
                    .iter()
                    .filter(|turn| active_turn_ids.contains(&turn.id))
                    .cloned()
                    .collect::<Vec<_>>()
            }
            None => history.turns.clone(),
        };
        let page = match paginate(&turns, &params.page, |turn| u64::from(turn.sequence)) {
            Ok(page) => page,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: page,
        })
        .expect("serialize session/turns/list response")
    }

    pub(crate) async fn handle_session_items_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionItemsListParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/items/list params: {error}"),
                );
            }
        };
        let history = match self
            .load_canonical_history(&request_id, params.session_id)
            .await
        {
            Ok(history) => history,
            Err(response) => return response,
        };
        // The rollout is a linear journal: after a rewind it still holds the
        // abandoned branch. Clients page this method to render the current
        // conversation, so scope to the active path (linear fallback for
        // tree-less histories — single-line sessions and forks).
        let mut items: Vec<ItemEnvelope> =
            active_path_items(&history).unwrap_or_else(|| history.items.clone());
        if let Some(turn_id) = params.turn_id {
            items.retain(|item| item.turn_id == turn_id);
        }
        let page = match paginate(&items, &params.page, |item| item.seq) {
            Ok(page) => page,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: page,
        })
        .expect("serialize session/items/list response")
    }

    /// Resolves the session's rollout path and reads its canonical history.
    /// The error variant is a ready-made JSON-RPC error response (session
    /// not found, or a damaged/unreadable file).
    pub(crate) async fn load_canonical_history(
        &self,
        request_id: &serde_json::Value,
        session_id: devo_protocol::native::ids::SessionId,
    ) -> Result<Arc<devo_core::CanonicalHistory>, serde_json::Value> {
        let Some(rollout_path) = self.resolve_rollout_path(&session_id).await else {
            return Err(self.error_response(
                request_id.clone(),
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            ));
        };
        self.history_cache
            .load(session_id, rollout_path)
            .await
            .map_err(|error| {
                self.error_response(
                    request_id.clone(),
                    ProtocolErrorCode::InternalError,
                    format!("failed to read session history: {error}"),
                )
            })
    }

    /// Finds the rollout file for a session, loaded or cold. Prefer durable
    /// indexes (SQLite, file scan) so history reads never depend on a
    /// mailbox round-trip. Fall back to the live actor record when the
    /// session is loaded and the index has no path yet.
    pub(crate) async fn resolve_rollout_path(
        &self,
        session_id: &devo_protocol::native::ids::SessionId,
    ) -> Option<PathBuf> {
        if let Ok(Some(index)) = self.deps.db.get_session_index(session_id)
            && let Some(path) = index.rollout_path
        {
            return Some(path);
        }
        if let Some(path) = self
            .rollout_store
            .find_rollout_by_session_id(session_id)
            .ok()
            .flatten()
        {
            return Some(path);
        }
        if let Some(handle) = self.session(*session_id).await
            && let Some(path) = handle.rollout_path().await.flatten()
        {
            return Some(path);
        }
        None
    }
}

/// Slices `items` (ascending by position) into one page.
///
/// A missing cursor starts forward pagination; a decimal cursor continues
/// after that position. `tail` starts at the newest page and `before:<position>`
/// continues toward older history. Both directions return chronological pages;
/// clients treat returned continuation cursors as opaque. The limit defaults to 50 and clamps into
/// `1..=200` — out-of-range limits never error (01 §4.2).
fn paginate<T: Clone>(
    items: &[T],
    params: &PageParams,
    position: impl Fn(&T) -> u64,
) -> Result<Page<T>, String> {
    // Native clients can open the recent tail without downloading the full
    // transcript. Older pages retain chronological order within each page.
    if params.cursor.as_deref() == Some("tail")
        || params
            .cursor
            .as_deref()
            .is_some_and(|cursor| cursor.starts_with("before:"))
    {
        let before = match params.cursor.as_deref() {
            Some("tail") => None,
            Some(cursor) => Some(
                cursor["before:".len()..]
                    .parse::<u64>()
                    .map_err(|_| "malformed cursor".to_string())?,
            ),
            None => unreachable!("tail pagination requires a cursor"),
        };
        let limit = params
            .limit
            .unwrap_or(DEFAULT_PAGE_LIMIT)
            .clamp(1, MAX_PAGE_LIMIT) as usize;
        let end = before.map_or(items.len(), |before| {
            items.partition_point(|item| position(item) < before)
        });
        let start = end.saturating_sub(limit);
        let data = items[start..end].to_vec();
        let next_cursor = (start > 0).then(|| format!("before:{}", position(&items[start])));
        return Ok(Page { data, next_cursor });
    }
    let after = match &params.cursor {
        Some(cursor) => cursor
            .parse::<u64>()
            .map_err(|_| "malformed cursor".to_string())?,
        None => 0,
    };
    let limit = params
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .clamp(1, MAX_PAGE_LIMIT) as usize;
    let data: Vec<T> = items
        .iter()
        .filter(|item| position(item) > after)
        .take(limit)
        .cloned()
        .collect();
    let last_position = data.last().map(&position).unwrap_or(after);
    let next_cursor = items
        .iter()
        .any(|item| position(item) > last_position)
        .then(|| last_position.to_string());
    Ok(Page { data, next_cursor })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn params(cursor: Option<&str>, limit: Option<u32>) -> PageParams {
        PageParams {
            cursor: cursor.map(str::to_owned),
            limit,
        }
    }

    #[test]
    fn paginate_walks_all_pages_without_gaps_or_duplicates() {
        let items: Vec<u64> = (1..=5).collect();
        let first = paginate(&items, &params(None, Some(2)), |item| *item).expect("page 1");
        assert_eq!(first.data, vec![1, 2]);
        assert_eq!(first.next_cursor.as_deref(), Some("2"));
        let second = paginate(&items, &params(Some("2"), Some(2)), |item| *item).expect("page 2");
        assert_eq!(second.data, vec![3, 4]);
        assert_eq!(second.next_cursor.as_deref(), Some("4"));
        let third = paginate(&items, &params(Some("4"), Some(2)), |item| *item).expect("page 3");
        assert_eq!(
            third,
            Page {
                data: vec![5],
                next_cursor: None,
            }
        );
    }

    #[test]
    fn paginate_defaults_and_clamps_the_limit() {
        let items: Vec<u64> = (1..=250).collect();
        let defaulted = paginate(&items, &params(None, None), |item| *item).expect("default");
        assert_eq!(defaulted.data.len(), 50);
        let clamped = paginate(&items, &params(None, Some(1000)), |item| *item).expect("clamp");
        assert_eq!(clamped.data.len(), 200);
        assert_eq!(clamped.next_cursor.as_deref(), Some("200"));
        let zero = paginate(&items, &params(None, Some(0)), |item| *item).expect("zero");
        assert_eq!(zero.data.len(), 1);
    }

    #[test]
    fn paginate_rejects_malformed_cursor() {
        let error = paginate(&[1u64], &params(Some("not-a-cursor"), None), |item| *item)
            .expect_err("must fail");
        assert_eq!(error, "malformed cursor");
    }

    #[test]
    fn paginate_empty_input_has_no_cursor() {
        let page = paginate(&Vec::<u64>::new(), &params(None, None), |item| *item).expect("empty");
        assert_eq!(
            page,
            Page {
                data: Vec::new(),
                next_cursor: None,
            }
        );
    }

    #[test]
    fn tail_pages_walk_backwards_across_sequence_gaps() {
        let items = [1u64, 3, 10, 20, 25];
        let mut cursor = "tail".to_string();
        for expected in [
            Page {
                data: vec![20, 25],
                next_cursor: Some("before:20".into()),
            },
            Page {
                data: vec![3, 10],
                next_cursor: Some("before:3".into()),
            },
            Page {
                data: vec![1],
                next_cursor: None,
            },
        ] {
            let actual =
                paginate(&items, &params(Some(&cursor), Some(2)), |item| *item).expect("tail page");
            assert_eq!(actual, expected);
            cursor = actual.next_cursor.unwrap_or_default();
        }
    }

    #[test]
    fn tail_pages_clamp_limits_and_handle_empty_boundaries() {
        let items: Vec<u64> = (1..=250).collect();
        for (cursor, limit, expected) in [
            (
                "tail",
                1000,
                Page {
                    data: (51..=250).collect(),
                    next_cursor: Some("before:51".into()),
                },
            ),
            (
                "tail",
                0,
                Page {
                    data: vec![250],
                    next_cursor: Some("before:250".into()),
                },
            ),
            (
                "before:1",
                10,
                Page {
                    data: vec![],
                    next_cursor: None,
                },
            ),
            (
                "before:0",
                10,
                Page {
                    data: vec![],
                    next_cursor: None,
                },
            ),
        ] {
            assert_eq!(
                paginate(&items, &params(Some(cursor), Some(limit)), |item| *item).expect("page"),
                expected
            );
        }
        assert_eq!(
            paginate(&[] as &[u64], &params(Some("tail"), None), |item| *item).expect("empty tail"),
            Page {
                data: vec![],
                next_cursor: None
            }
        );
        assert_eq!(
            paginate(&items, &params(Some("before:bad"), None), |item| *item),
            Err("malformed cursor".into())
        );
    }
}
