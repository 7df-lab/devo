# Verification Traceability Matrix

| Test Reference | Test Type | Test Location | Directly Verifies | Verified Revision | Derived Coverage | Notes |
|---|---|---|---|---:|---|---|
| bottom_pane::reference_popup::tests::empty_query_orders_skill_mcp_then_file | Unit | crates/tui/src/bottom_pane/reference_popup.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies empty-query result category ordering. |
| bottom_pane::reference_popup::tests::non_empty_query_filters_all_categories_in_order | Unit | crates/tui/src/bottom_pane/reference_popup.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies combined fuzzy filtering across skills, MCP, and files. |
| bottom_pane::reference_popup::tests::selected_mcp_reference_uses_stable_insert_token | Unit | crates/tui/src/bottom_pane/reference_popup.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies MCP reference token and mention target creation. |
| bottom_pane::reference_popup::tests::stale_file_results_are_ignored | Unit | crates/tui/src/bottom_pane/reference_popup.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies stale file-search snapshot rejection. |
| bottom_pane::chat_composer::reference_popup_tests::bare_at_opens_reference_popup | Unit | crates/tui/src/bottom_pane/chat_composer.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies bare `@` opens reference search and requests an empty query. |
| bottom_pane::chat_composer::reference_popup_tests::at_inside_normal_text_opens_reference_popup | Unit | crates/tui/src/bottom_pane/chat_composer.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies TUI token-local `@` in normal composer text. |
| bottom_pane::chat_composer::reference_popup_tests::at_inside_slash_command_args_opens_reference_popup | Unit | crates/tui/src/bottom_pane/chat_composer.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies TUI token-local `@` in slash-command arguments. |
| bottom_pane::chat_composer::reference_popup_tests::esc_dismissal_does_not_reopen_until_token_changes | Unit | crates/tui/src/bottom_pane/chat_composer.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies Escape dismissal preserves text and does not reopen for the same token. |
| bottom_pane::chat_composer::reference_popup_tests::enter_with_no_selected_reference_does_not_submit | Unit | crates/tui/src/bottom_pane/chat_composer.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies Enter does not submit while the reference popup has no focused result. |
| interactive::tests::file_search_request_creates_and_updates_active_session | Unit | crates/tui/src/interactive.rs | L2-DES-CLIENT-002 | 1 | L1-REQ-CLIENT-004 | Verifies host file-search request/update wiring, including empty query as search rather than cancel. |
