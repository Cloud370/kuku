pub mod event {
    pub use kuku::event::*;
}

#[allow(dead_code)]
#[path = "../src/skill/lifecycle.rs"]
mod lifecycle;

use kuku::event::{
    ExecutionScope, SkillLoadFact, SkillLoadOrigin, SourceFact, SourceScope, WorkspaceRelativePath,
};
use lifecycle::SkillLifecycle;

fn execution() -> ExecutionScope {
    serde_json::from_value(serde_json::json!({
        "workspace_id": "wsp_000000000000000000000000",
        "task_id": "tsk_000000000000000000000000",
        "run_id": "run_000000000000000000000000",
        "turn_id": "trn_000000000000000000000000",
        "conversation_id": "con_000000000000000000000000",
        "turn_index": 0
    }))
    .unwrap()
}

fn fact(skill_id: &str, origin: SkillLoadOrigin) -> SkillLoadFact {
    SkillLoadFact {
        execution: execution(),
        caused_by_request_id: None,
        skill_id: skill_id.to_owned(),
        source: SourceFact {
            scope: SourceScope::Project,
            id: format!("source:{skill_id}"),
            relative_path: Some(
                WorkspaceRelativePath::parse(".agents/skills/tdd/SKILL.md").unwrap(),
            ),
        },
        origin,
        content_hash: format!("sha256:{skill_id}"),
    }
}

#[test]
fn origins_are_explicit_and_loaded_skill_cannot_unload() {
    let state = SkillLifecycle::reduce([
        fact("skill:project:manual", SkillLoadOrigin::You),
        fact("skill:project:agent", SkillLoadOrigin::Agent),
        fact("skill:project:bootstrap", SkillLoadOrigin::Bootstrap),
        fact("skill:project:project", SkillLoadOrigin::Project),
    ]);

    assert_eq!(4, state.loaded().len());
    assert_eq!(SkillLoadOrigin::You, state.loaded()[0].origin);
    assert_eq!(SkillLoadOrigin::Agent, state.loaded()[1].origin);
    assert_eq!(SkillLoadOrigin::Bootstrap, state.loaded()[2].origin);
    assert_eq!(SkillLoadOrigin::Project, state.loaded()[3].origin);
    assert!(!state.can_unload("skill:project:manual"));
}

#[test]
fn repeated_load_is_append_only_but_state_keeps_first_origin() {
    let state = SkillLifecycle::reduce([
        fact("skill:project:tdd", SkillLoadOrigin::Bootstrap),
        fact("skill:project:tdd", SkillLoadOrigin::Agent),
    ]);

    assert_eq!(2, state.facts().len());
    assert_eq!(1, state.loaded().len());
    assert_eq!(SkillLoadOrigin::Bootstrap, state.loaded()[0].origin);
}

#[test]
fn only_local_staged_entries_are_removable() {
    let state = SkillLifecycle::reduce([fact("skill:project:loaded", SkillLoadOrigin::Project)]);

    assert!(state.can_remove_staged("skill:project:staged", true));
    assert!(!state.can_remove_staged("skill:project:staged", false));
    assert!(!state.can_remove_staged("skill:project:loaded", true));
}
