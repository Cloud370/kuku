use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::{backend::TestBackend, Terminal};

use crate::menu::{Choice, PickerKind};

use super::{app, cell, click, key, render};

#[test]
fn spacious_composer_and_picker_use_responsive_insets() {
    let mut app = app();
    app.pane_mut().draft.insert("甲");
    let mut wide = Terminal::new(TestBackend::new(128, 32)).unwrap();
    let mut compact = Terminal::new(TestBackend::new(44, 12)).unwrap();
    let wide_frame = wide.draw(|frame| render(frame, &mut app)).unwrap();
    let (wide_x, wide_y) = cell(wide_frame.buffer, "甲");
    assert_eq!(wide_x, 6);
    assert_ne!(
        wide_frame.buffer[(wide_x, wide_y + 1)].bg,
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
