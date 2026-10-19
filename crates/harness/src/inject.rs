//! Host helper that loads H and formats the prompt digest.
//!
//! Call sites: cold prompt assembly (after catalog skills, before hidden goal).
//! See L2-DES-HARNESS-001 DD-5 and docs/rlm-native-api.md section 7.

use std::path::Path;

use crate::digest::format_harness_state_for_prompt;
use crate::state::{HarnessState, HarnessStateError};

/// Loads session harness state and returns the model-visible digest string.
///
/// Missing state yields an empty string. Corrupt JSON fails closed ([`Err`]);
/// callers should omit the digest rather than invent content.
#[derive(Debug, Default, Clone, Copy)]
pub struct HarnessDigestInjector;

impl HarnessDigestInjector {
    /// Load `session_dir/harness/harness_state.json` and format for prompt.
    pub fn digest_for_session_dir(session_dir: &Path) -> Result<String, HarnessStateError> {
        let path = HarnessState::file_path(session_dir);
        if !path.exists() {
            return Ok(String::new());
        }
        let state = HarnessState::load(&path)?;
        Ok(format_harness_state_for_prompt(&state))
    }

    /// Best-effort load: missing or corrupt yields empty (caller may log).
    pub fn digest_or_empty(session_dir: &Path) -> String {
        Self::digest_for_session_dir(session_dir).unwrap_or_default()
    }

    /// Session digest merged with the global harness store.
    ///
    /// Global entries are cross-session guidance; without them in the digest
    /// a session never sees memories refined with `global_=True` (they sit on
    /// disk unseen). Global entries are labeled `(global)` by the formatter;
    /// on an id collision the session-local entry wins (more specific).
    pub fn digest_or_empty_with_global(session_dir: &Path, global_dir: &Path) -> String {
        let local = Self::load_state_or_default(session_dir);
        let global = Self::load_state_or_default(global_dir);
        let merged = local.merged_with_global(&global);
        format_harness_state_for_prompt(&merged)
    }

    fn load_state_or_default(dir: &Path) -> HarnessState {
        let path = HarnessState::file_path(dir);
        if !path.exists() {
            return HarnessState::default();
        }
        HarnessState::load(&path).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{HarnessEntry, HarnessKind, HarnessScope};
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-HARNESS-001
    /// Verifies: injector returns digest content after a successful state write.
    #[test]
    fn injector_loads_digest_for_prompt_site() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(HarnessDigestInjector::digest_or_empty(dir.path()), "");

        let path = HarnessState::file_path(dir.path());
        let mut state = HarnessState::default();
        let now = Utc::now();
        state.entries.get_mut("memory").unwrap().insert(
            "m1".into(),
            HarnessEntry {
                id: "m1".into(),
                kind: HarnessKind::Memory,
                title: "pref".into(),
                content: "prefer tabs".into(),
                path: None,
                scope: Some(HarnessScope::Local),
                reference: serde_json::json!({}),
                arguments: serde_json::json!({}),
                metadata: serde_json::json!({}),
                source: "test".into(),
                created_at: now,
                updated_at: now,
                version: 1,
            },
        );
        state.save_atomic(&path).unwrap();

        let digest = HarnessDigestInjector::digest_for_session_dir(dir.path()).unwrap();
        assert!(digest.contains("prefer tabs"));
        assert!(digest.contains("refine.run"));
        assert!(digest.contains("Continual harness") || digest.contains("harness-digest"));
    }

    /// Trace: bug #46 — global harness entries must surface in every digest.
    /// Verifies: merged digest includes both scopes, labels global entries,
    /// and prefers the session entry on id collision.
    #[test]
    fn merged_digest_includes_global_entries() {
        let session_dir = tempfile::tempdir().unwrap();
        let global_dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        let make_entry = |id: &str, title: &str, scope: HarnessScope| HarnessEntry {
            id: id.into(),
            kind: HarnessKind::Memory,
            title: title.into(),
            content: format!("content of {id}"),
            path: None,
            scope: Some(scope),
            reference: serde_json::json!({}),
            arguments: serde_json::json!({}),
            metadata: serde_json::json!({}),
            source: "test".into(),
            created_at: now,
            updated_at: now,
            version: 1,
        };

        let mut local = HarnessState::default();
        local.entries.get_mut("memory").unwrap().insert(
            "local_mem".into(),
            make_entry("local_mem", "Local memory", HarnessScope::Local),
        );
        local.entries.get_mut("memory").unwrap().insert(
            "shared_id".into(),
            make_entry("shared_id", "Session wins", HarnessScope::Local),
        );
        local
            .save_atomic(&HarnessState::file_path(session_dir.path()))
            .unwrap();

        let mut global = HarnessState::default();
        global.entries.get_mut("memory").unwrap().insert(
            "global_mem".into(),
            make_entry("global_mem", "Global memory", HarnessScope::Global),
        );
        global.entries.get_mut("memory").unwrap().insert(
            "shared_id".into(),
            make_entry("shared_id", "Global shadowed", HarnessScope::Global),
        );
        global
            .save_atomic(&HarnessState::file_path(global_dir.path()))
            .unwrap();

        let digest = HarnessDigestInjector::digest_or_empty_with_global(
            session_dir.path(),
            global_dir.path(),
        );
        assert!(digest.contains("Local memory"));
        assert!(digest.contains("Global memory (global)"));
        assert!(digest.contains("Session wins"));
        assert!(!digest.contains("Global shadowed"));
    }

    /// Trace: bug #46 — missing global store degrades to the local digest.
    #[test]
    fn merged_digest_without_global_store_is_local_only() {
        let session_dir = tempfile::tempdir().unwrap();
        let empty_global = tempfile::tempdir().unwrap();
        assert_eq!(
            HarnessDigestInjector::digest_or_empty_with_global(
                session_dir.path(),
                empty_global.path()
            ),
            ""
        );
    }
}
