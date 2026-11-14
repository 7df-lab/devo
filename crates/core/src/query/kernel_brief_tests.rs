//! End-to-end query assembly with a real kernel and a capturing provider.

use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn kernel_brief_is_fresh_for_primary_requests_but_absent_from_reusable_prefixes() {
    let source =
        devo_kernel::default_runtime_pythonpath(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("runtime source");
    let kernel = devo_kernel::KernelSession::spawn(devo_kernel::KernelSessionConfig {
        python: std::path::PathBuf::from("python"),
        python_path_entries: vec![source],
        ..devo_kernel::KernelSessionConfig::default()
    })
    .await
    .expect("real runtime");
    let mut fixtures = CapturingFixtures::new("kernel-brief-capturing-provider");
    fixtures.runtime = ToolRuntime::new_with_context(
        Arc::clone(&fixtures.registry),
        PermissionChecker::always_allow(),
        ToolRuntimeContext {
            kernel: Some(Arc::clone(&kernel)),
            ..ToolRuntimeContext::default()
        },
    );
    let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
    session.push_message(Message::user(
        CompactionSummary::new("Earlier work.").render(),
    ));
    let last: SharedLastModelRequest = Arc::new(Mutex::new(None));
    for (code, expected_name, absent_name) in [
        (
            "previous_inventory_binding = object()",
            "previous_inventory_binding",
            "new_inventory_binding",
        ),
        (
            "del previous_inventory_binding; new_inventory_binding = object()",
            "new_inventory_binding",
            "previous_inventory_binding",
        ),
    ] {
        assert_eq!(
            kernel.execute(code).await.expect("change namespace").status,
            "ok"
        );
        session.push_message(Message::user("continue"));
        fixtures
            .query(
                &mut session,
                &TurnConfig::new(Model::default(), /*reasoning_effort_selection*/ None),
                QueryOptions {
                    last_model_request: Some(Arc::clone(&last)),
                    ..QueryOptions::default()
                },
                /*callback*/ None,
            )
            .await
            .expect("query");
        let sent = fixtures
            .requests
            .lock()
            .expect("requests")
            .last()
            .expect("primary request")
            .clone();
        let cached = last
            .lock()
            .expect("prefix")
            .clone()
            .expect("cached request");
        let mut expected_cache = sent.clone();
        let mut briefs = Vec::new();
        expected_cache.messages.retain(|message| {
            if let [RequestContent::Text { text }] = message.content.as_slice()
                && text.starts_with("<live_python_names>")
            {
                briefs.push(text.clone());
                false
            } else {
                true
            }
        });
        assert_eq!(briefs.len(), 1);
        assert!(briefs[0].contains(expected_name));
        assert!(!briefs[0].contains(absent_name));
        // Auto-review and cell-watch extend this exact cached object. It must
        // retain the rest of the prefix but never a transient live inventory.
        assert_eq!(json!(cached), json!(expected_cache));
        assert!(
            !json!(session.prompt_source_messages())
                .to_string()
                .contains("<live_python_names>")
        );
    }
    assert_eq!(fixtures.requests.lock().expect("requests").len(), 2);
    kernel.shutdown().await.expect("shutdown");
}
