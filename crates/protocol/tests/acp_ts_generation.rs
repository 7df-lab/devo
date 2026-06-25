#[test]
fn generated_acp_typescript_contains_wire_discriminants_and_names() {
    let output = devo_protocol::acp_ts::generate_acp_typescript();

    assert!(output.contains("export type JsonValue"));
    assert!(output.contains("export type AcpSessionNotification"));
    assert!(output.contains("sessionId"));
    assert!(output.contains("\"sessionUpdate\": \"agent_message_chunk\""));
    assert!(output.contains("\"sessionUpdate\": \"config_option_update\""));
    assert!(output.contains("export type AcpContentBlock"));
    assert!(output.contains("\"type\": \"text\""));
    assert!(output.contains("currentValue"));
    assert!(!output.contains("current_value"));
    assert!(output.contains("export type AcpRequestPermissionParams"));
    assert!(output.contains("export type RequestUserInputRespondParams"));
    assert!(output.contains("request_id"));
}

#[test]
fn generated_protocol_typescript_contains_non_acp_client_method_roots() {
    let output = devo_protocol::acp_ts::generate_protocol_typescript();

    assert!(output.contains("export type GoalStatusParams"));
    assert!(output.contains("export type GoalStatusResult"));
    assert!(output.contains("export type SkillListParams"));
    assert!(output.contains("export type SkillListResult"));
    assert!(output.contains("export type CommandExecParams"));
    assert!(output.contains("export type EventsSubscribeParams"));
    assert!(output.contains("session_id"));
    assert!(output.contains("threadId"));
}

#[test]
fn generated_protocol_schema_contains_method_bindings() {
    let output = devo_protocol::acp_ts::generate_protocol_schema_json();
    let value: serde_json::Value = serde_json::from_str(&output).expect("schema JSON");

    assert_eq!(
        value["methods"]["session/update"]["incomingNotification"],
        "AcpSessionNotification"
    );
    assert_eq!(
        value["methods"]["session/prompt"]["outgoingRequest"],
        "AcpPromptParams"
    );
    assert_eq!(
        value["methods"]["goal/status"]["outgoingRequest"],
        "GoalStatusParams"
    );
    assert_eq!(
        value["methods"]["skills/list"]["incomingResult"],
        "SkillListResult"
    );
    assert!(value["schemas"]["AcpSessionNotification"].is_object());
    assert!(value["schemas"]["GoalStatusParams"].is_object());
    assert!(value["schemas"]["SkillListResult"].is_object());
}
