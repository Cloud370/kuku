use kuku_server::api::AgentThread;

#[test]
fn agent_thread_fixture_is_read_only_projection_data() {
    let thread: AgentThread =
        serde_json::from_str(include_str!("fixtures/context/v1/agent_thread.json")).unwrap();
    assert_eq!(
        "con_000000000000000000000002",
        thread.conversation_id.as_str()
    );
    assert!(thread.messages.is_empty());
    assert!(!serde_json::to_string(&thread).unwrap().contains("input"));
}
