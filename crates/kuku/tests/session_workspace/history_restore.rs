use super::{
    completed_turns, edit_owned, file_tool_options, result_for, start, until, write_owned,
};
use kuku::{Input, Session};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_restore_applies_only_the_approved_native_file_versions() {
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let session = start(file_tool_options(
        &work,
        &data,
        vec![
            vec![write_owned("base", "base\n")],
            vec![],
            vec![edit_owned("edit", "base", "new\nextra")],
            vec![(
                "create",
                "write_file",
                json!({"path":"created.txt","content":"a\nb\n"}),
            )],
            vec![(
                "overlap",
                "write_file",
                json!({"path":"overlap.txt","content":"native\n"}),
            )],
            vec![(
                "script",
                "run_js",
                json!({"code":r#"
await tools.read_file({path:"overlap.txt"});
const edited = await tools.edit_file({path:"overlap.txt",old_text:"native",new_text:"script"});
const created = await tools.write_file({path:"script-only.txt",content:"script-only\n"});
return [edited.status, created.status];
"#}),
            )],
            vec![],
        ],
    ))
    .await;
    let target = until(|| {
        session
            .history_points("main", u64::MAX, 1)
            .pop()
            .filter(|point| point.can_continue)
    })
    .await;
    session
        .send(Input::new(
            "later",
            "main",
            "modify files after the base turn",
        ))
        .unwrap();
    completed_turns(&session, 2).await;
    let latest = until(|| {
        session
            .history_points("main", u64::MAX, 1)
            .pop()
            .filter(|point| point.can_continue && point.number == 2)
    })
    .await;
    for call in ["base", "edit", "create", "overlap"] {
        assert_eq!("ok", result_for(&session, call).0, "{call}");
    }
    let path = |name: &str| work.path().join(name);
    let read = |name: &str| std::fs::read_to_string(path(name)).unwrap();
    assert_eq!(read("owned.txt"), "new\nextra\n");
    assert_eq!(read("overlap.txt"), "script\n");
    assert_eq!(read("script-only.txt"), "script-only\n");

    let preview = || {
        Session::preview_file_rewind(session.events_path(), work.path(), "main", target.event_id)
    };
    let plan = preview().unwrap();
    assert_eq!(
        plan.changes
            .iter()
            .map(|change| (change.path.as_str(), change.remove, change.lines))
            .collect::<Vec<_>>(),
        [
            ("created.txt", true, Some((0, 2))),
            ("owned.txt", false, Some((1, 2)))
        ]
    );
    assert_eq!(plan.skipped, ["overlap.txt"]);

    session
        .jump_history("conversation-only", "main", target.event_id)
        .unwrap();
    assert_eq!(read("owned.txt"), "new\nextra\n");
    assert_eq!(read("created.txt"), "a\nb\n");
    session
        .jump_history("return-to-later", "main", latest.event_id)
        .unwrap();

    std::fs::write(path("owned.txt"), "edit on disk after preview\n").unwrap();
    let jump = |id| {
        session.jump_history_with_file_restore(id, "main", target.event_id, Some(&plan.fingerprint))
    };
    assert!(jump("stale-preview")
        .unwrap_err()
        .to_string()
        .contains("file plan changed"));
    assert_eq!(read("created.txt"), "a\nb\n");
    assert_eq!(read("owned.txt"), "edit on disk after preview\n");
    std::fs::write(path("owned.txt"), "new\nextra\n").unwrap();

    let receipt = jump("approved").unwrap();
    assert_eq!(jump("approved").unwrap(), receipt);
    assert_eq!(read("owned.txt"), "base\n");
    assert!(!path("created.txt").exists());
    assert_eq!(read("overlap.txt"), "script\n");
    assert_eq!(read("script-only.txt"), "script-only\n");
    session.close().await.unwrap();
}
