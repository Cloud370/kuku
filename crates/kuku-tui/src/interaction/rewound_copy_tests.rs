use std::path::PathBuf;

use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use kuku_client::{HistoryEntry, HistoryKind, HistoryPage, RewindGroup, RewindGroupPage};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::app::App;
use crate::jobs::{Job, Update};

fn connected_app() -> App {
    let workspace = PathBuf::from("/workspace");
    let mut app = App::new(workspace.clone(), None);
    app.update(Update::Connected {
        endpoint: "http://127.0.0.1:6789".into(),
        workspace,
    });
    app.effects.clear();
    app
}

#[test]
fn rewind_group_row_inset_preserves_multibyte_copy() {
    let mut app = connected_app();
    app.pane_mut().transcript.replace_recent(HistoryPage {
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
    let request = app.pane_mut().transcript.toggle_rewind_group(50).unwrap();
    app.pane_mut().transcript.apply_rewind_group_page(
        &request,
        Ok(RewindGroupPage {
            entries: vec![HistoryEntry {
                id: 3,
                agent: "main".into(),
                turn: Some(1),
                kind: HistoryKind::User,
                text: Some("界面🙂 copied text".into()),
                turn_summary: None,
                details: Default::default(),
            }],
            next_before: None,
        }),
    );
    app.effects.clear();

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let buffer = terminal
        .draw(|frame| crate::ui::render(frame, &mut app))
        .unwrap()
        .buffer;
    let width = usize::from(buffer.area.width);
    let (row, column) = buffer
        .content
        .chunks(width)
        .enumerate()
        .find_map(|(row, cells)| {
            let line = cells
                .iter()
                .map(|cell| cell.symbol())
                .collect::<Vec<_>>()
                .join("");
            line.find("User ·")
                .map(|byte| (row as u16, line[..byte].chars().count() as u16))
        })
        .expect("rendered rewound user entry");
    let send = |app: &mut App, kind, column| {
        app.event(
            Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }),
            buffer,
        );
    };
    send(&mut app, MouseEventKind::Down(MouseButton::Left), column);
    send(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        column + 40,
    );
    send(&mut app, MouseEventKind::Up(MouseButton::Left), column + 40);

    let copied = app
        .effects
        .iter()
        .find_map(|job| match job {
            Job::Copy(text) => Some(text.as_str()),
            _ => None,
        })
        .expect("drag copies the selected text");
    assert_eq!("User · 界面🙂 copied text", copied);
}
