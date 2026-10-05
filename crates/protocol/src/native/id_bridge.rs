//! Helpers for extracting UUID bytes from opaque Native IDs.
//!
//! `SessionId` / `TurnId` / `ItemId` are interned opaque IDs (`ses_` /
//! `turn_` / `item_`, or bare hyphenated UUIDs for pre-v2 data). Most call
//! sites should keep the opaque ID. Use these helpers only when a boundary
//! still needs raw [`Uuid`] bytes (for example ACP adapters).

use uuid::Uuid;

use super::ids::{ItemId, SessionId, TurnId};

fn parse_opaque_or_bare(raw: &str, prefix: &str) -> Option<Uuid> {
    if let Ok(uuid) = Uuid::parse_str(raw) {
        return Some(uuid);
    }
    let stripped = raw.strip_prefix(prefix)?;
    Uuid::parse_str(stripped).ok()
}

/// Extract UUID bytes from a session id (bare or `ses_`-prefixed).
pub fn uuid_from_session_id(session_id: &SessionId) -> Option<Uuid> {
    parse_opaque_or_bare(session_id.as_str(), "ses_")
}

/// Extract UUID bytes from a turn id (bare or `turn_`-prefixed).
pub fn uuid_from_turn_id(turn_id: &TurnId) -> Option<Uuid> {
    parse_opaque_or_bare(turn_id.as_str(), "turn_")
}

/// Extract UUID bytes from an item id (bare or `item_`-prefixed).
pub fn uuid_from_item_id(item_id: &ItemId) -> Option<Uuid> {
    parse_opaque_or_bare(item_id.as_str(), "item_")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn prefixed_native_new_yields_uuid_bytes() {
        let session = SessionId::new();
        let turn = TurnId::new();
        let item = ItemId::new();
        assert!(session.as_str().starts_with("ses_"));
        assert!(turn.as_str().starts_with("turn_"));
        assert!(item.as_str().starts_with("item_"));
        assert!(uuid_from_session_id(&session).is_some());
        assert!(uuid_from_turn_id(&turn).is_some());
        assert!(uuid_from_item_id(&item).is_some());
    }

    #[test]
    fn bare_hyphenated_uuid_still_parses() {
        let uuid = Uuid::now_v7();
        let session = SessionId::from_legacy_uuid(uuid);
        assert_eq!(uuid_from_session_id(&session), Some(uuid));
    }
}
