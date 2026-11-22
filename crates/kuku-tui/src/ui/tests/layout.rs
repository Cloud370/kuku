use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::style::{Color, Modifier};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::json;

use crate::menu::{Choice, PickerKind};

use super::{app, cell, click, key, render, text_position};

#[test]
fn turns_share_the_user_surface_and_edit_aligned_card_connectors() {
    let mut app = app();
    for mut entry in [
        json!({"id": 1, "type": "user", "turn": 1, "turn_number": 1, "text": "甲", "ts": "2026-11-23T09:00:00+08:00"}),
        json!({"id": 2, "type": "tool_call", "turn": 1, "tool_call_id": "edit", "tool": "edit_file", "args": {"path": "guide.md"}, "ts": "2026-11-23T01:00:01Z"}),
        json!({"id": 3, "type": "tool_result", "turn": 1, "tool_call_id": "edit", "tool": "edit_file", "status": "ok", "ts": "2026-11-23T01:00:01.024Z", "structured": {"kind": "file_edit", "path": "guide.md", "change": {
            "added_lines": 1, "removed_lines": 1, "preview": "-先\n+后", "preview_line_numbers": [7, 7],
        }}}),
        json!({"id": 4, "type": "compaction", "turn": 1, "request_id": "compact-1", "status": "completed", "elapsed_ms": 2340, "ts": "2026-11-23T01:00:04Z"}),
        json!({"id": 5, "type": "assistant", "turn": 1, "text": "乙\n\n```rust\nfn main() {}\n```"}),
        json!({"id": 6, "type": "user", "turn": 2, "turn_number": 2, "text": "丙"}),
        json!({"id": 7, "type": "assistant", "turn": 2, "text": "丁"}),
    ] {
        entry["agent"] = json!("main");
        if entry["turn"] == 1 {
            entry["turn_started_at_ms"] = json!(1_795_395_600_000u64);
        }
        app.pane_mut()
            .transcript
            .apply(serde_json::from_value(entry).unwrap(), None);
    }
    let mut terminal = Terminal::new(TestBackend::new(96, 44)).unwrap();
    let frame = terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let buffer = frame.buffer;
    let session = crate::theme::session_area(buffer.area);
    let left = session.x;
    let x = left + 4;
    let (_, first_prompt) = text_position(buffer, "Turn 1");
    let (_, last_reply) = cell(buffer, "丁");
    for y in first_prompt..=last_reply {
        let blank = (session.left()..session.right()).all(|x| buffer[(x, y)].symbol() == " ");
        let background = if blank {
            Color::Reset
        } else {
            crate::theme::SURFACE
        };
        for x in session.left()..session.right() {
            assert_eq!(background, buffer[(x, y)].bg, "background at {x},{y}");
        }
    }
    for text in ["甲", "乙", "丙", "丁"] {
        let (_, y) = cell(buffer, text);
        assert_eq!("│", buffer[(left, y)].symbol());
        assert_eq!(" ", buffer[(left - 2, y)].symbol());
        assert_ne!("│", buffer[(x, y)].symbol());
    }
    let (removed, removed_row) = cell(buffer, "先");
    let (added, added_row) = cell(buffer, "后");
    for (column, row, marker, color) in [
        (removed, removed_row, "-", crate::theme::ERROR),
        (added, added_row, "+", crate::theme::SUCCESS),
    ] {
        assert_eq!("│", buffer[(left, row)].symbol());
        assert_eq!("│", buffer[(x, row)].symbol());
        assert_eq!("7", buffer[(column - 4, row)].symbol());
        assert_eq!(marker, buffer[(column - 2, row)].symbol());
        assert_eq!(color, buffer[(column - 2, row)].fg);
        assert_eq!(" ", buffer[(column - 1, row)].symbol());
    }
    let (_, compaction) = text_position(buffer, "Context compacted");
    assert_eq!("╭", buffer[(left, compaction)].symbol());
    let (_, edit) = text_position(buffer, "guide.md");
    let (_, reply) = text_position(buffer, "Assistant");
    for heading in [edit, compaction, reply] {
        let gap = heading - 1;
        assert_eq!("╭", buffer[(left, heading)].symbol());
        assert_ne!("│", buffer[(x, heading)].symbol());
        assert_eq!(" ", buffer[(left, gap)].symbol());
        assert_eq!("│", buffer[(x, gap)].symbol());
        assert_eq!(crate::theme::TURN_RULE, buffer[(x, gap)].fg);
    }
    for (heading, metric) in [
        (edit, "01:00:00Z · T+0s"),
        (compaction, "01:00:01Z · T+1s · 24ms"),
        (reply, "01:00:04Z · T+4s · 2.3s"),
    ] {
        let (column, row) = text_position(buffer, metric);
        assert_eq!((x + 2, heading - 1), (column, row));
    }
    let (_, second_turn) = text_position(buffer, "Turn 2");
    assert_eq!(" ", buffer[(x, second_turn - 1)].symbol());
    assert_eq!(Color::Reset, buffer[(x, second_turn - 1)].bg);
}

#[test]
fn spacious_composer_and_picker_use_responsive_insets() {
    let mut app = app();
    app.pane_mut().draft.insert("甲");
    let mut wide = Terminal::new(TestBackend::new(128, 32)).unwrap();
    let mut compact = Terminal::new(TestBackend::new(44, 12)).unwrap();
    let wide_frame = wide.draw(|frame| render(frame, &mut app)).unwrap();
    let (wide_x, wide_y) = cell(wide_frame.buffer, "甲");
    assert_eq!(wide_x, 7);
    assert_eq!(
        wide_frame.buffer[(wide_x, wide_y + 1)].bg,
        crate::theme::SURFACE
    );
    assert_eq!(wide_frame.buffer[(wide_x, wide_y + 1)].symbol(), " ");
    assert_ne!(
        wide_frame.buffer[(wide_x, wide_y + 2)].bg,
        crate::theme::SURFACE
    );
    let compact_frame = compact.draw(|frame| render(frame, &mut app)).unwrap();
    let (compact_x, compact_y) = cell(compact_frame.buffer, "甲");
    assert!(compact_x < wide_x);
    assert_eq!(compact_y, 9);

    app.picker(PickerKind::Commands, Choice::commands());
    let frame = wide.draw(|frame| render(frame, &mut app)).unwrap();
    let (x, y) = cell(frame.buffer, "›");
    assert_ne!(frame.buffer[(x, y)].bg, frame.buffer[(x - 1, y)].bg);
    assert!(!frame.buffer[(x, y)].modifier.contains(Modifier::DIM));
    assert!(!frame.buffer[(0, 0)].modifier.contains(Modifier::DIM));
}

#[test]
fn composer_click_places_the_caret_in_scrolled_and_wrapped_unicode_text() {
    let mut app = app();
    let mut terminal = Terminal::new(TestBackend::new(30, 20)).unwrap();
    let prefix = "earlier\n".repeat(6) + &"x".repeat(24);
    app.pane_mut()
        .draft
        .insert(&format!("{prefix}中文e\u{301}👨‍👩‍👧‍👦end"));
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let (x, y) = cell(buffer, "中");
    assert!(y > app.composer_area.y);
    app.event(
        Event::Resize(buffer.area.width, buffer.area.height + 8),
        buffer,
    );
    app.event(Event::Resize(buffer.area.width, buffer.area.height), buffer);
    click(&mut app, buffer, (x + 1, y));
    key(&mut app, KeyCode::Char('!'), KeyModifiers::NONE);
    assert_eq!(
        app.pane().draft.text(),
        format!("{prefix}!中文e\u{301}👨‍👩‍👧‍👦end")
    );
    let buffer = terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap()
        .buffer;
    let point = cell(buffer, "👨‍👩‍👧‍👦");
    click(&mut app, buffer, point);
    key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
    assert_eq!(
        app.pane().draft.text(),
        format!("{prefix}!中文e\u{301}?👨‍👩‍👧‍👦end")
    );
}
