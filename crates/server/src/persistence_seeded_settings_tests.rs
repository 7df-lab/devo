use std::sync::Arc;

use devo_core::CanonicalHistory;
use devo_core::InternalRecord;
use devo_core::SessionSettingsField;
use devo_core::rollout::ROLLOUT_FORMAT_VERSION;
use devo_core::rollout::RolloutLine;
use pretty_assertions::assert_eq;

use super::settings_test_record;
use crate::persistence::ReplayState;
use crate::persistence::RolloutStore;
use crate::persistence::parse_rollout_line;
use crate::replay_hydrate::ReplayedSession;

#[test]
fn seeded_settings_preserve_initial_version_and_first_mutation_epoch() {
    #[derive(Debug)]
    enum Writer {
        Live,
        Rehydrated,
    }

    for writer in [Writer::Live, Writer::Rehydrated] {
        let (dir, store, record) = settings_test_record();
        let native = devo_core::native_session_from_record(&record).expect("native session");
        let extras = devo_core::session_persistence_extras_from_record(&record);
        // Deliberately differ from the header so replay must retain every field,
        // even though all three initial settings share the same epoch.
        let settings = [
            (
                SessionSettingsField::Model,
                serde_json::json!("seeded-model"),
            ),
            (
                SessionSettingsField::ModelBindingId,
                serde_json::json!("seeded-binding"),
            ),
            (
                SessionSettingsField::SandboxProfile,
                serde_json::json!("strict"),
            ),
        ];
        store
            .append_seeded_session_meta_create_at(
                &record.rollout_path,
                &native,
                extras.clone(),
                &settings,
            )
            .expect("create seeded journal");

        let read_lines = || {
            std::fs::read_to_string(&record.rollout_path)
                .expect("read seeded journal")
                .lines()
                .map(|raw| {
                    let mut line = parse_rollout_line(raw).expect("parse seeded journal");
                    // Wall-clock timestamps are not part of this regression.
                    match &mut line {
                        RolloutLine::Internal { timestamp, .. }
                        | RolloutLine::SessionMeta { timestamp, .. } => {
                            *timestamp = record.created_at;
                        }
                        other => panic!("unexpected seeded journal line: {other:?}"),
                    }
                    line
                })
                .collect::<Vec<_>>()
        };
        let mut expected_lines = settings
            .iter()
            .map(|(field, value)| RolloutLine::Internal {
                v: ROLLOUT_FORMAT_VERSION,
                timestamp: record.created_at,
                session_id: native.id,
                turn_id: None,
                seq: 0,
                entry: InternalRecord::SessionSettings {
                    schema_version: 1,
                    field: *field,
                    value: value.clone(),
                    epoch: 0,
                },
            })
            .collect::<Vec<_>>();
        let mut expected_header = native.clone();
        expected_header.version = 1;
        expected_lines.push(RolloutLine::SessionMeta {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: record.created_at,
            session: Box::new(expected_header.clone()),
            extras: Some(Box::new(extras.clone())),
        });
        let initial_lines = read_lines();
        assert_eq!(initial_lines, expected_lines, "initial {writer:?} journal");

        let mut replay = ReplayState::default();
        for line in initial_lines {
            replay
                .apply_rollout_line(line)
                .expect("replay initial settings");
        }
        let mut replayed = replay.session.take().expect("replayed session");
        replay.apply_session_settings(&mut replayed);
        let mut expected_replayed = expected_header.clone();
        expected_replayed.model.model = "seeded-model".into();
        expected_replayed.model.provider = "seeded-binding".into();
        assert_eq!(
            (replayed, replay.sandbox_profile_override()),
            (
                ReplayedSession::from_native(expected_replayed, Some(extras), record.created_at),
                Some("strict".to_string()),
            ),
            "all epoch-zero fields must survive {writer:?} replay"
        );
        assert_eq!(
            devo_core::read_canonical_history(&record.rollout_path)
                .expect("read initial canonical history"),
            CanonicalHistory {
                session: Some(Box::new(expected_header.clone())),
                ..CanonicalHistory::default()
            },
            "initial {writer:?} header must remain at version one"
        );

        let store = match writer {
            Writer::Live => store,
            Writer::Rehydrated => {
                let previous_engine = Arc::downgrade(
                    &store
                        .shared_journal(&record.rollout_path)
                        .expect("live journal engine"),
                );
                drop(store);
                assert!(
                    previous_engine.upgrade().is_none(),
                    "fresh writer must rehydrate rather than reuse the process-global engine"
                );
                RolloutStore::new(dir.path().to_path_buf(), /*event_log*/ None)
            }
        };
        store
            .append_session_settings_batch_at(
                &record.rollout_path,
                record.id,
                /*_settings_epoch*/ 1,
                &[(
                    SessionSettingsField::Model,
                    serde_json::json!("updated-model"),
                )],
            )
            .expect("append first real settings mutation");
        expected_lines.push(RolloutLine::Internal {
            v: ROLLOUT_FORMAT_VERSION,
            timestamp: record.created_at,
            session_id: native.id,
            turn_id: None,
            seq: 0,
            entry: InternalRecord::SessionSettings {
                schema_version: 1,
                field: SessionSettingsField::Model,
                value: serde_json::json!("updated-model"),
                epoch: 1,
            },
        });
        assert_eq!(read_lines(), expected_lines, "first {writer:?} mutation");

        expected_header.version = 2;
        expected_header.model.model = "updated-model".into();
        assert_eq!(
            devo_core::read_canonical_history(&record.rollout_path)
                .expect("read canonical history after mutation"),
            CanonicalHistory {
                session: Some(Box::new(expected_header)),
                ..CanonicalHistory::default()
            },
            "first {writer:?} mutation must advance version one to two"
        );
    }
}
