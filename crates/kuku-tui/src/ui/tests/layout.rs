use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::{backend::TestBackend, Terminal};

use super::{app, cell, key, render};

#[test]
fn spacious_composer_and_picker_use_responsive_insets() {
    let mut app = app();
    app.pane_mut().draft.insert("甲");
    let mut wide = Terminal::new(TestBackend::new(128, 32)).unwrap();
    let mut compact = Terminal::new(TestBackend::new(44, 12)).unwrap();
    let wide_frame = wide.draw(|frame| render(frame, &mut app)).unwrap();
    let (wide_x, wide_y) = cell(wide_frame.buffer, "甲");
    assert!(wide_x >= 6);
    assert_eq!(
        wide_frame.buffer[(wide_x, wide_y + 1)].bg,
        crate::theme::SURFACE
    );
    let compact_frame = compact.draw(|frame| render(frame, &mut app)).unwrap();
    let (compact_x, compact_y) = cell(compact_frame.buffer, "甲");
    assert!(compact_x < wide_x);
    assert_eq!(compact_y, 9);

    key(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
    let frame = wide.draw(|frame| render(frame, &mut app)).unwrap();
    let (x, y) = cell(frame.buffer, "›");
    assert_ne!(frame.buffer[(x, y)].bg, frame.buffer[(x - 1, y)].bg);
    assert!(!frame.buffer[(x, y)].modifier.contains(Modifier::DIM));
    assert!(frame.buffer[(0, 0)].modifier.contains(Modifier::DIM));
}
