use super::*;
use chrono::Utc;
use devo_harness::{
    HarnessDigestInjector, HarnessEntry, RefineEdit, apply_proposal_re_read,
    bind_proposal_baseline, rollback_refinement,
};
use pretty_assertions::assert_eq;

fn memory(id: &str, content: &str) -> HarnessEntry {
    let now = Utc::now();
    HarnessEntry {
        id: id.into(),
        kind: HarnessKind::Memory,
        title: id.into(),
        content: content.into(),
        path: None,
        scope: Some(HarnessScope::Local),
        reference: json!({}),
        arguments: json!({}),
        metadata: json!({}),
        source: "test".into(),
        created_at: now,
        updated_at: now,
        version: 1,
    }
}

fn proposal(id: &str, source: &str) -> RefineProposal {
    let mut after = memory(
        "model_id",
        "Check a narrow regression before running the full suite.",
    );
    after.metadata = json!({"source_id":source,"applicability":"When changing repository code"});
    RefineProposal {
        id: id.into(),
        trigger: "manual".into(),
        summary: "Reuse a tested workflow".into(),
        evidence: "session memory".into(),
        expected_outcome: "not evaluated".into(),
        edits: vec![RefineEdit {
            op: RefineEditOp::Create,
            kind: HarnessKind::Memory,
            id: after.id.clone(),
            before: None,
            after: Some(after),
        }],
    }
}

#[test]
fn local_evidence_promotes_without_mutation_and_survives_reload_across_sessions() {
    let session_a = tempfile::tempdir().unwrap();
    let session_b = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let local_path = HarnessState::file_path(session_a.path());
    let global_path = HarnessState::file_path(home.path());
    let mut local = HarnessState::default();
    let mut source = memory(
        "workflow",
        "A narrow regression exposed the failure before the full suite ran.",
    );
    source.path = Some("Devo".into());
    local
        .entries
        .get_mut("memory")
        .unwrap()
        .insert(source.id.clone(), source);
    // Exact duplicate content with another local ID must not produce colliding
    // host-derived global IDs or make the entire batch fail.
    let mut duplicate = local.entries["memory"]["workflow"].clone();
    duplicate.id = "workflow_duplicate".into();
    local
        .entries
        .get_mut("memory")
        .unwrap()
        .insert(duplicate.id.clone(), duplicate);
    local.save_atomic(&local_path).unwrap();
    let original_local = std::fs::read(&local_path).unwrap();
    let mut baseline = HarnessState::default();
    let mut existing = memory("existing_global", "Keep this independent preference.");
    existing.scope = Some(HarnessScope::Global);
    baseline
        .entries
        .get_mut("memory")
        .unwrap()
        .insert(existing.id.clone(), existing.clone());
    baseline.save_atomic(&global_path).unwrap();
    let candidates = Candidates::load(Some(session_a.path()), &baseline, "session-a").unwrap();
    assert_eq!(candidates.entries.len(), 1);
    let mut planned = proposal("promote_one", "workflow");
    candidates
        .constrain(&mut planned, &baseline, HarnessScope::Global)
        .unwrap();
    bind_proposal_baseline(&mut planned, &baseline).unwrap();
    let committed = apply_proposal_re_read(&global_path, &planned).unwrap();
    assert_eq!(committed.entries["memory"]["existing_global"], existing);
    assert_eq!(std::fs::read(&local_path).unwrap(), original_local);
    for session in [session_a.path(), session_b.path()] {
        let digest = HarnessDigestInjector::digest_or_empty_with_global(session, home.path());
        assert!(digest.contains("Project: Devo\n"));
        assert!(digest.contains("Check a narrow regression"));
    }
    assert!(
        Candidates::load(
            Some(session_a.path()),
            &HarnessState::load(&global_path).unwrap(),
            "session-a"
        )
        .unwrap()
        .entries
        .is_empty()
    );
    // A concurrent plan based on the same old baseline cannot overwrite/repeat.
    let mut concurrent = planned.clone();
    concurrent.id = "concurrent".into();
    assert!(apply_proposal_re_read(&global_path, &concurrent).is_err());
    // Existing transactional rollback recovers just the promoted row, not local
    // inputs or independently maintained global entries.
    let (rolled, _) = rollback_refinement(&global_path, "promote_one", "undo_one").unwrap();
    assert_eq!(rolled.entries["memory"], baseline.entries["memory"]);
    assert_eq!(std::fs::read(&local_path).unwrap(), original_local);
}

#[test]
fn unsafe_sources_and_scope_escalation_are_not_promoted() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = HarnessState::default();
    let mut untrusted = memory("trajectory", "Unverified lesson");
    untrusted.metadata = json!({"trust":"unverified_trajectory_lesson"});
    for entry in [
        untrusted,
        memory("secret", "password = unsafe"),
        memory("safe", "Useful repeatable observation"),
    ] {
        state
            .entries
            .get_mut("memory")
            .unwrap()
            .insert(entry.id.clone(), entry);
    }
    state
        .save_atomic(&HarnessState::file_path(dir.path()))
        .unwrap();
    let baseline = HarnessState::default();
    let candidates = Candidates::load(Some(dir.path()), &baseline, "session-a").unwrap();
    assert_eq!(
        candidates.entries.keys().cloned().collect::<Vec<_>>(),
        vec!["safe"]
    );
    assert!(
        candidates
            .constrain(
                &mut proposal("unknown", "trajectory"),
                &baseline,
                HarnessScope::Global
            )
            .is_err()
    );
    assert!(
        candidates
            .constrain(
                &mut proposal("local", "safe"),
                &baseline,
                HarnessScope::Local
            )
            .is_err()
    );
    let mut prompt = proposal("prompt", "safe");
    prompt.edits[0].kind = HarnessKind::Prompt;
    assert!(
        candidates
            .constrain(&mut prompt, &baseline, HarnessScope::Global)
            .is_err()
    );
    let mut update = proposal("update", "safe");
    update.edits[0].op = RefineEditOp::Update;
    assert!(
        candidates
            .constrain(&mut update, &baseline, HarnessScope::Global)
            .is_err()
    );
    let mut direct = proposal("direct", "safe");
    direct.edits[0].after.as_mut().unwrap().metadata = json!({"source":"refine_llm"});
    assert!(
        candidates
            .constrain(&mut direct, &baseline, HarnessScope::Global)
            .is_err()
    );
    let mut deletion = proposal("delete", "safe");
    deletion.edits[0].op = RefineEditOp::Delete;
    deletion.edits[0].after = None;
    assert!(
        candidates
            .constrain(&mut deletion, &baseline, HarnessScope::Global)
            .is_err()
    );
}

#[test]
fn consolidation_retains_host_provenance() {
    let mut baseline = HarnessState::default();
    let mut before = memory("lesson", "Original wording");
    before.source = "session_harness_refine".into();
    before.metadata = json!({"source":"session_harness_refine","session_source_id":"source","source_session_id":"session-a","session_source_fingerprint":"fingerprint","evaluation":"not_evaluated"});
    baseline
        .entries
        .get_mut("memory")
        .unwrap()
        .insert(before.id.clone(), before.clone());
    let mut update = proposal("update", "source");
    update.edits[0].op = RefineEditOp::Update;
    update.edits[0].id = "lesson".into();
    let after = update.edits[0].after.as_mut().unwrap();
    after.id = "lesson".into();
    after.metadata = json!({"source":"refine_llm"});
    preserve_provenance(&mut update, &baseline);
    let after = update.edits[0].after.as_ref().unwrap();
    assert_eq!(after.metadata, before.metadata);
    assert_eq!(after.source, before.source);
    assert_eq!(
        after.content,
        "Check a narrow regression before running the full suite."
    );
}
