use kuku::{query, Query};

#[test]
fn query_returns_typed_builder() {
    let builder = query("inspect this project");
    assert_eq!(builder.prompt(), "inspect this project");
}

#[test]
fn query_builder_can_set_session() {
    let builder = Query::new("continue").session("s_001");
    assert_eq!(builder.prompt(), "continue");
    assert_eq!(builder.session_id(), Some("s_001"));
}
