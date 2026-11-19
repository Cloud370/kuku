use std::collections::VecDeque;

use kuku_client::{HistoryEntry, HistoryKind, HistoryPage, RewindGroup, RewindGroupPage};
use serde_json::json;

use super::{Anchor, AnchorLine, HistoryRequest, Key, ReplySource, RewindGroupAction, Transcript};

fn rows(transcript: &mut Transcript) -> Vec<String> {
    transcript
        .project(44, 10)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn all_rows(transcript: &mut Transcript) -> Vec<String> {
    transcript
        .project(80, 40)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn rewind_group_expands_in_place_without_entering_active_transcript_state() {
    let entry = |id, kind, text: &str| HistoryEntry {
        id,
        agent: "main".into(),
        turn: Some(1),
        kind,
        text: Some(text.into()),
        turn_summary: None,
        details: [
            ("turn_number".into(), json!(1)),
            ("summary".into(), json!(text)),
        ]
        .into_iter()
        .collect(),
    };
    let group = RewindGroup {
        rewind_event_id: 50,
        target_event_id: 2,
        agent: "main".into(),
        turn_number: Some(1),
        entry_count: 4,
    };
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: vec![
            entry(1, HistoryKind::User, "active prompt"),
            entry(8, HistoryKind::Assistant, "surviving reply"),
            entry(9, HistoryKind::User, "later prompt"),
        ],
        rewind_groups: vec![group.clone()],
        next_before: Some(3),
    });
    assert_eq!(3, transcript.recent.records.len());
    assert_eq!(
        Some(ReplySource::Local("surviving reply")),
        transcript.last_reply()
    );
    assert_eq!(
        vec![
            (9, 1, "later prompt".into()),
            (1, 1, "active prompt".into())
        ],
        transcript.rewind_messages("main")
    );
    let collapsed = all_rows(&mut transcript).join("\n");
    assert!(collapsed.contains("Rewound content · read-only · excluded from model context"));
    assert!(!collapsed.contains("hidden prompt"));
    assert!(!collapsed.contains("hidden answer"));
    assert!(
        collapsed.find("active prompt").unwrap()
            < collapsed
                .find("Rewound content · read-only · excluded from model context")
                .unwrap()
    );
    transcript.reading = Some(super::Reading {
        window: transcript.recent.clone(),
        anchor: Anchor {
            key: Key::RewindGroup(50),
            line: AnchorLine::Group(0),
        },
        newer: VecDeque::new(),
        unseen: false,
        manual: true,
    });
    transcript.apply_page(
        HistoryRequest::Older(3),
        HistoryPage {
            entries: Vec::new(),
            rewind_groups: vec![group],
            next_before: None,
        },
    );
    assert_eq!(
        1,
        transcript
            .reading
            .as_ref()
            .unwrap()
            .window
            .rewind_groups
            .len()
    );
    let anchored_rows = rows(&mut transcript);
    let header_row = anchored_rows
        .iter()
        .position(|row| row.contains("Rewound content"))
        .unwrap();
    let anchored_header = anchored_rows[header_row].clone();
    assert_eq!(
        Some((50, RewindGroupAction::Toggle)),
        transcript.rewind_group_action_at(header_row)
    );

    let request = transcript.toggle_rewind_group(50).unwrap();
    assert_eq!(None, request.before);
    assert!(transcript.apply_rewind_group_page(&request, Err("offline".into())));
    let error_rows = all_rows(&mut transcript).join("\n");
    assert!(error_rows.contains("offline"));
    let retry = transcript.retry_rewind_group(50).unwrap();
    assert_eq!(None, retry.before);
    let mut hidden_answer = entry(3, HistoryKind::Assistant, "hidden answer");
    hidden_answer
        .details
        .insert("reasoning_preview".into(), json!("hidden thinking"));
    assert!(transcript.apply_rewind_group_page(
        &retry,
        Ok(RewindGroupPage {
            entries: vec![
                hidden_answer,
                entry(4, HistoryKind::ToolResult, "hidden tool result"),
                entry(5, HistoryKind::Status, "hidden status"),
            ],
            next_before: Some(3),
        }),
    ));
    transcript.reading.as_mut().unwrap().anchor = Anchor {
        key: Key::RewindGroup(50),
        line: AnchorLine::RewindEntry {
            event_id: 4,
            line: 0,
        },
    };
    let anchored_entry = rows(&mut transcript)[0].clone();
    let more = transcript.load_more_rewind_group(50).unwrap();
    assert_eq!(Some(3), more.before);
    assert!(transcript.apply_rewind_group_page(
        &more,
        Ok(RewindGroupPage {
            entries: vec![entry(2, HistoryKind::User, "hidden prompt")],
            next_before: None,
        }),
    ));
    assert_eq!(anchored_entry, rows(&mut transcript)[0]);
    transcript.reading.as_mut().unwrap().anchor = Anchor {
        key: Key::RewindGroup(50),
        line: AnchorLine::Group(0),
    };

    let expanded_rows = all_rows(&mut transcript);
    let expanded = expanded_rows.join("\n");
    let group_header = expanded
        .find("Rewound content · read-only · excluded from model context")
        .unwrap();
    let hidden_prompt = expanded.find("hidden prompt").unwrap();
    let hidden_answer = expanded.find("hidden answer").unwrap();
    let hidden_tool = expanded.find("hidden tool result").unwrap();
    let hidden_status = expanded.find("hidden status").unwrap();
    let surviving_reply = expanded.find("surviving reply").unwrap();
    let later_prompt = expanded.find("later prompt").unwrap();
    assert!(group_header < hidden_prompt);
    assert!(hidden_prompt < hidden_answer);
    assert!(hidden_answer < hidden_tool);
    assert!(hidden_tool < hidden_status);
    assert!(hidden_status < surviving_reply);
    assert!(surviving_reply < later_prompt);
    assert!(!expanded.contains("hidden thinking"));
    assert_eq!(3, transcript.recent.records.len());
    assert_eq!(
        Some(ReplySource::Local("surviving reply")),
        transcript.last_reply()
    );
    assert_eq!(
        vec![
            (9, 1, "later prompt".into()),
            (1, 1, "active prompt".into())
        ],
        transcript.rewind_messages("main")
    );
    let expanded_visible = rows(&mut transcript);
    let expanded_header = expanded_visible
        .iter()
        .find(|row| row.contains("Rewound content"))
        .unwrap();
    assert_eq!(&anchored_header, expanded_header);
    assert_eq!(
        Some(50),
        transcript
            .rewind_group_choices()
            .first()
            .map(|group| group.rewind_event_id)
    );
    assert!(transcript.toggle_rewind_group(50).is_none());
    let collapsed_rows = rows(&mut transcript);
    let collapsed_header = collapsed_rows
        .iter()
        .find(|row| row.contains("Rewound content"))
        .unwrap();
    assert_eq!(&anchored_header, collapsed_header);
    assert!(!rows(&mut transcript).join("\n").contains("hidden prompt"));
    assert_eq!(
        Some(ReplySource::Local("surviving reply")),
        transcript.last_reply()
    );
}

#[test]
fn rewind_group_page_from_before_a_transcript_refresh_is_discarded() {
    let group = RewindGroup {
        rewind_event_id: 50,
        target_event_id: 2,
        agent: "main".into(),
        turn_number: None,
        entry_count: 1,
    };
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: Vec::new(),
        rewind_groups: vec![group],
        next_before: None,
    });
    assert!(all_rows(&mut transcript)
        .join("\n")
        .contains("Rewound content · read-only · excluded from model context"));
    let request = transcript.toggle_rewind_group(50).unwrap();
    transcript.replace_recent(HistoryPage {
        entries: Vec::new(),
        rewind_groups: Vec::new(),
        next_before: None,
    });

    assert!(!transcript.apply_rewind_group_page(
        &request,
        Ok(RewindGroupPage {
            entries: vec![HistoryEntry {
                id: 2,
                agent: "main".into(),
                turn: Some(1),
                kind: HistoryKind::User,
                text: Some("stale hidden content".into()),
                turn_summary: None,
                details: Default::default(),
            }],
            next_before: None,
        }),
    ));
    assert!(!all_rows(&mut transcript)
        .join("\n")
        .contains("stale hidden content"));
}

#[test]
fn rewind_group_visual_cap_is_explicit() {
    let group = RewindGroup {
        rewind_event_id: 50,
        target_event_id: 2,
        agent: "main".into(),
        turn_number: None,
        entry_count: 1,
    };
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: Vec::new(),
        rewind_groups: vec![group],
        next_before: None,
    });
    let request = transcript.toggle_rewind_group(50).unwrap();
    assert!(transcript.apply_rewind_group_page(
        &request,
        Ok(RewindGroupPage {
            entries: vec![HistoryEntry {
                id: 2,
                agent: "main".into(),
                turn: Some(1),
                kind: HistoryKind::Assistant,
                text: Some("long content ".repeat(10_000)),
                turn_summary: None,
                details: Default::default(),
            }],
            next_before: None,
        }),
    ));

    transcript.reading = Some(super::Reading {
        window: transcript.recent.clone(),
        anchor: Anchor {
            key: Key::RewindGroup(50),
            line: AnchorLine::Group(usize::MAX),
        },
        newer: VecDeque::new(),
        unseen: false,
        manual: true,
    });
    assert!(rows(&mut transcript)
        .join("\n")
        .contains("Display limit reached"));
}

#[test]
fn rewind_group_page_window_is_bounded_and_marks_discarded_newer_entries() {
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: Vec::new(),
        rewind_groups: vec![RewindGroup {
            rewind_event_id: 200,
            target_event_id: 2,
            agent: "main".into(),
            turn_number: None,
            entry_count: 60,
        }],
        next_before: None,
    });

    let mut next_before = None;
    for page in 0..5u64 {
        let request = if page == 0 {
            transcript.toggle_rewind_group(200).unwrap()
        } else {
            transcript.load_more_rewind_group(200).unwrap()
        };
        assert_eq!(next_before, request.before);
        let first_id = 138 - page * 12;
        let entries = (first_id..first_id + 12)
            .map(|id| HistoryEntry {
                id,
                agent: "main".into(),
                turn: Some(1),
                kind: HistoryKind::User,
                text: Some(format!("page-{page}")),
                turn_summary: None,
                details: Default::default(),
            })
            .collect();
        next_before = Some(first_id);
        assert!(transcript.apply_rewind_group_page(
            &request,
            Ok(RewindGroupPage {
                entries,
                next_before,
            }),
        ));
    }

    transcript.reading = Some(super::Reading {
        window: transcript.recent.clone(),
        anchor: Anchor {
            key: Key::RewindGroup(200),
            line: AnchorLine::Group(0),
        },
        newer: VecDeque::new(),
        unseen: false,
        manual: true,
    });
    let rendered = all_rows(&mut transcript).join("\n");
    assert!(rendered.contains("page-4"));
    assert!(!rendered.contains("page-0"));
    assert!(rendered.contains("Newer entries are not retained in this bounded view"));

    assert!(transcript.toggle_rewind_group(200).is_none());
    let restart = transcript.toggle_rewind_group(200).unwrap();
    assert_eq!(None, restart.before);
}

#[test]
fn rewind_group_only_window_can_scroll_with_empty_active_history() {
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: Vec::new(),
        rewind_groups: vec![RewindGroup {
            rewind_event_id: 50,
            target_event_id: 2,
            agent: "main".into(),
            turn_number: None,
            entry_count: 1,
        }],
        next_before: None,
    });
    let request = transcript.toggle_rewind_group(50).unwrap();
    assert!(transcript.apply_rewind_group_page(
        &request,
        Ok(RewindGroupPage {
            entries: vec![HistoryEntry {
                id: 2,
                agent: "main".into(),
                turn: Some(1),
                kind: HistoryKind::Assistant,
                text: Some("hidden line\n".repeat(80)),
                turn_summary: None,
                details: Default::default(),
            }],
            next_before: None,
        }),
    ));

    assert!(transcript.scroll(-1, 80, 10).is_none());
    assert!(transcript.is_reading());
    assert!(matches!(
        transcript.reading.as_ref().unwrap().anchor.line,
        AnchorLine::RewindEntry { event_id: 2, .. }
    ));
}

#[test]
fn group_only_history_window_trims_and_can_page_back_to_newer_groups() {
    let groups = (1..=super::MAX_RECORDS as u64 + 1)
        .map(|target_event_id| RewindGroup {
            rewind_event_id: target_event_id + 1_000,
            target_event_id,
            agent: "main".into(),
            turn_number: None,
            entry_count: 0,
        })
        .collect();
    let mut window = super::Window::from_page(HistoryPage {
        entries: Vec::new(),
        rewind_groups: groups,
        next_before: Some(1),
    });

    let request = window.trim_back().unwrap();

    assert_eq!(super::MAX_RECORDS, window.rewind_groups.len());
    assert_eq!(
        HistoryRequest::Newer {
            first: super::MAX_RECORDS as u64 + 1,
            before: super::MAX_RECORDS as u64 + 2,
        },
        request
    );
}
