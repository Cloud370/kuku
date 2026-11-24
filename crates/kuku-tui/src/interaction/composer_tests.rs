use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use kuku_client::{Action, HistoryKind, HistoryPage, OutputReference, Timing};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use ratatui::Terminal;
use serde_json::json;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, KeyboardFocus, Modal};
use crate::jobs::{Job, Update};
use crate::menu::PickerKind;
use crate::setup::ThinkingDefaults;

use super::tests::{connected_app, insert_message, press};

fn load_model(app: &mut App, applied: Option<&str>, next: Option<&str>) {
    let selection = |effort| {
        json!({
            "connection": "fixture",
            "protocol": "openai-responses",
            "model": "gemini-4-neon",
            "context_window_tokens": 100_000,
            "max_output_tokens": 4_000,
            "settings": {"reasoning_effort": effort},
            "capabilities": {
                "reasoning_efforts": ["low", "medium", "high", "high", "max"],
                "profile": "openrouter",
                "service_tiers": ["priority", "provider-custom"],
                "service_tier_source": "model_list",
                "reasoning_default": {"value": "max", "source": "model_list"}
            },
            "tools": [],
        })
    };
    let config = |effort| json!({"tier":"fixture-model", "reasoning_effort":effort});
    let snapshot = serde_json::from_value(json!({
        "session_id": app.active.session,
        "workspace": app.workspace,
        "cursor": 52,
        "pending": [],
        "agents": [],
        "selection": selection(applied.filter(|effort| *effort != "provider-default")),
        "pending_selection": next.map(|effort| selection(Some(effort))),
        "agent": {
            "address": app.active.agent,
            "config": config(next.or(applied)),
            "active_config": next.map(|_| config(applied)),
            "status": "running",
            "turn": 10,
            "pending_inputs": 0,
            "execution_id": 52,
            "usage": {"model_requests":0,"unreported_requests":0},
        }
    }))
    .unwrap();
    app.update(Update::Loaded(
        app.selection(),
        Box::new(snapshot),
        HistoryPage {
            entries: Vec::new(),
            rewind_groups: Vec::new(),
            next_before: None,
        },
    ));
    app.update(Update::WatchConnected(app.selection()));
    app.effects.clear();
}

fn action_picker(app: &mut App, query: &str) {
    app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    press(app, KeyCode::Char('/'));
    app.paste(query);
}

fn frame(app: &mut App, width: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, 28)).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, app))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn lines(buffer: &Buffer) -> Vec<String> {
    buffer
        .content
        .chunks(usize::from(buffer.area.width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

fn position(buffer: &Buffer, text: &str) -> (u16, u16) {
    lines(buffer)
        .iter()
        .enumerate()
        .find_map(|(row, line)| {
            line.find(text)
                .map(|index| (line[..index].width() as u16, row as u16))
        })
        .unwrap_or_else(|| panic!("Missing {text} in\n{}", lines(buffer).join("\n")))
}

#[test]
fn slash_deletion_closes_the_list_without_changing_the_draft() {
    let mut app = connected_app();
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Char('m'));
    press(&mut app, KeyCode::Backspace);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker)) if picker.query.text() == "/"));
    press(&mut app, KeyCode::Backspace);
    assert!(app.modal.is_none());
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.pane().draft.text(), "n");

    action_picker(&mut app, "model");
    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Delete);
    assert!(app.modal.is_none());
    assert_eq!(app.pane().draft.text(), "n");
}

#[test]
fn command_and_model_lists_wrap_and_stay_above_the_composer() {
    let mut app = connected_app();
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Up);
    assert!(
        matches!(&app.modal, Some(Modal::Picker(picker)) if matches!(
            picker.selected_choice(), Some(crate::menu::ChoiceKind::Command(crate::menu::CommandMenu::Quit))
        ))
    );
    press(&mut app, KeyCode::Down);
    let buffer = frame(&mut app, 120);
    assert_eq!(
        position(&buffer, "Actions and Skills").1,
        position(&buffer, "Actions and Skills   /").1
    );
    assert!(lines(&buffer).iter().any(|line| line.contains("›/new")));
    press(&mut app, KeyCode::Esc);

    app.pane_mut().draft.set_text("keep this draft".into());
    app.open_models();
    press(&mut app, KeyCode::Up);
    let buffer = frame(&mut app, 60);
    assert!(lines(&buffer)
        .iter()
        .any(|line| line.contains("›Set current model as default")));
    assert_eq!(
        position(&buffer, "Model route").1,
        position(&buffer, "Search models…").1
    );
    assert!(position(&buffer, "d Default + switch").1 < position(&buffer, "keep this draft").1);
    press(&mut app, KeyCode::Down);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker)) if picker.selected == 0));
}

#[test]
fn jobs_alias_preserves_arguments_through_completion_paste_and_execution() {
    let mut app = connected_app();
    app.pane_mut().exists = true;
    press(&mut app, KeyCode::Char('/'));
    for character in "job  cancel JOB-8".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    press(&mut app, KeyCode::Tab);
    assert_eq!("/jobs  cancel JOB-8", app.pane().draft.text());
    assert!(app.modal.is_none());
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.effects.last(), Some(Job::CancelBackground(selection, id))
        if *selection == app.selection() && id == "JOB-8")
    );
    assert!(app.alias_hint.is_none());

    app.paste("/job cancel JOB-9");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.effects.last(), Some(Job::CancelBackground(_, id)) if id == "JOB-9"));
    assert_eq!("/job → /jobs", app.alias_hint.take().unwrap().text);
    app.paste("/job cancel JOB-10");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.effects.last(), Some(Job::CancelBackground(_, id)) if id == "JOB-10"));
    assert!(app.alias_hint.is_none());

    app.effects.clear();
    app.paste("/job cancel");
    press(&mut app, KeyCode::Enter);
    assert_eq!("/job cancel", app.pane().draft.text());
    assert_eq!("Use /jobs or /jobs cancel <id>", app.notice.unwrap().text);
    assert!(app.effects.is_empty());
}

#[test]
fn bare_exit_words_only_open_local_confirmation_on_both_submit_keys() {
    for (text, modifiers) in [
        ("  exit  ", KeyModifiers::NONE),
        ("  quit ", KeyModifiers::ALT),
    ] {
        let mut app = connected_app();
        load_model(&mut app, Some("low"), None);
        app.paste(text);
        press(&mut app, KeyCode::Left);
        let cursor = app.pane().draft.cursor();
        app.key(KeyEvent::new(KeyCode::Enter, modifiers));
        assert!(matches!(app.modal, Some(Modal::ExitConfirmation)));
        app.key(KeyEvent {
            kind: KeyEventKind::Repeat,
            ..KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        });
        assert!(!app.quit);
        for width in [60, 120] {
            let buffer = frame(&mut app, width);
            let (x, y) = position(&buffer, "Exit kuku? Y exit · Esc edit");
            assert_eq!(app.composer_area.x, x);
            assert!(y < app.composer_area.y && app.composer_area.y - y <= 4);
        }
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
        assert!(!app.quit);
        assert_eq!(text, app.pane().draft.text());
        assert_eq!(cursor, app.pane().draft.cursor());
        assert_eq!(Some(52), app.pane().execution());
        assert!(app.pending.is_empty() && app.effects.is_empty());

        app.key(KeyEvent::new(KeyCode::Enter, modifiers));
        press(&mut app, KeyCode::Char('Y'));
        assert!(app.quit);
        assert!(app.pending.is_empty() && app.effects.is_empty());
    }
}

#[test]
fn exit_phrases_multiline_and_attached_input_still_submit() {
    for (text, attached) in [
        ("explain exit", false),
        ("quit\n# code", false),
        ("EXIT", false),
        ("/unrecognized", false),
        ("/quit with arguments", false),
        ("quit", true),
    ] {
        let mut app = connected_app();
        app.paste(text);
        if attached {
            app.pane_mut().value_attachments.push((
                "notes".into(),
                OutputReference {
                    id: "saved-notes".into(),
                    offset: 0,
                    bytes: 12,
                },
            ));
        }
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert!(
            matches!(app.effects.as_slice(), [Job::Save(command)]
            if matches!(&command.action, Action::Send { text: sent, timing: Timing::NextTurn, values, .. }
                if sent == text && values.len() == usize::from(attached))),
            "{text}"
        );
    }
    for alias in ["/quit", "/exit", "/q"] {
        let mut app = connected_app();
        app.paste(alias);
        press(&mut app, KeyCode::Enter);
        assert!(app.quit && app.modal.is_none());
        assert!(app.alias_hint.is_none());
        assert!(app.effects.is_empty());
    }
}

#[test]
fn composer_lists_keep_alignment_draft_selection_and_reading_position() {
    for width in [60, 120] {
        let mut app = connected_app();
        load_model(&mut app, Some("low"), Some("medium"));
        for id in 1..30 {
            insert_message(
                &mut app,
                id,
                HistoryKind::Assistant,
                &format!("Reading line {id}"),
                Default::default(),
            );
        }
        app.paste("draft stays in place");
        press(&mut app, KeyCode::Left);
        let cursor = app.pane().draft.cursor();
        let baseline = frame(&mut app, width);
        assert!(lines(&baseline)
            .iter()
            .any(|line| line.contains("Running") && line.contains("Next Medium")));
        app.scroll(-6);
        assert_eq!(KeyboardFocus::Transcript, app.pane().keyboard_focus);
        let anchor = app.pane().transcript.reading_anchor().cloned();
        assert!(anchor.is_some());
        let (column, row) = position(&baseline, "draft stays in place");
        let button = MouseButton::Left;
        for (kind, col) in [
            (MouseEventKind::Down(button), column),
            (MouseEventKind::Drag(button), column + 4),
        ] {
            app.event(
                Event::Mouse(MouseEvent {
                    kind,
                    column: col,
                    row,
                    modifiers: KeyModifiers::NONE,
                }),
                &baseline,
            );
        }
        action_picker(&mut app, "models");
        let suggestions = frame(&mut app, width);
        let label = lines(&suggestions)
            .into_iter()
            .find(|line| line.contains("›/model "))
            .unwrap();
        assert_eq!(
            (column, row),
            position(&suggestions, "draft stays in place")
        );
        assert_eq!(column, position(&suggestions, "›/model ").0 + 1);
        assert!(label.trim_end().width() <= 76 + usize::from(column));
        let menu_row = position(&suggestions, "Actions and Skills").1;
        let right = width - crate::theme::horizontal_gutter(width) - 1;
        assert_eq!(suggestions[(right, menu_row)].bg, crate::theme::SURFACE);
        assert_eq!(suggestions[(right, row)].bg, crate::theme::SURFACE);
        assert!(suggestions[(column, row)]
            .modifier
            .contains(Modifier::REVERSED));
        press(&mut app, KeyCode::Esc);

        action_picker(&mut app, "thinking");
        press(&mut app, KeyCode::Enter);
        let Some(Job::ThinkingDefaults(ticket, ..)) = app.effects.last() else {
            panic!("The inherited value must resolve without the Agent override");
        };
        app.update(Update::ThinkingDefaults(
            *ticket,
            ThinkingDefaults {
                inherited: Some(
                    serde_json::from_value::<kuku::ProviderSettings>(
                        json!({"reasoning_effort":"low"}),
                    )
                    .unwrap(),
                ),
                save: None,
            },
        ));
        press(&mut app, KeyCode::Down);
        let thinking = frame(&mut app, width);
        let text = lines(&thinking).join("\n");
        assert_eq!(
            column,
            position(&thinking, "Use default · Model default · Low").0
        );
        assert!(text.contains("Medium ✓ · Agent override"), "{text}");
        assert!(text.contains("Provider declares · Max"), "{text}");
        assert!(text.contains("gemini-4-neon low"), "{text}");
        assert!(
            text.contains("Thinking · gemini-4-neon · Next request"),
            "{text}"
        );
        assert_eq!(1, text.matches("High").count());
        press(&mut app, KeyCode::Char('d'));
        assert!(
            matches!(&app.modal, Some(Modal::Picker(picker)) if picker.kind == PickerKind::Thinking
            && picker.thinking.as_ref().is_some_and(|thinking| thinking.confirm.is_none()))
        );
        press(&mut app, KeyCode::Esc);

        assert_eq!(KeyboardFocus::Transcript, app.pane().keyboard_focus);
        assert_eq!(anchor.as_ref(), app.pane().transcript.reading_anchor());
        assert_eq!(cursor, app.pane().draft.cursor());
        assert_eq!("draft stays in place", app.pane().draft.text());
        assert!(app.pending.is_empty());
    }
}

#[test]
fn cycling_respects_pending_model_settings_and_changes_only_the_active_agent() {
    let mut app = connected_app();
    load_model(&mut app, Some("low"), Some("high"));
    app.paste("draft to keep");
    press(&mut app, KeyCode::Left);
    app.focus_history();
    let cursor = app.pane().draft.cursor();
    let cycle = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
    app.key(cycle);
    assert!(matches!(app.effects.as_slice(), [Job::Save(command)]
        if command.target == app.selection().target &&
        matches!(&command.action, Action::Configure { config }
            if config.reasoning_effort.as_deref() == Some("max") && config.tier.as_deref() == Some("fixture-model"))));
    app.key(cycle);
    assert_eq!(1, app.effects.len());
    assert!(app.notice.is_none());
    assert_eq!(KeyboardFocus::Transcript, app.pane().keyboard_focus);
    assert_eq!(cursor, app.pane().draft.cursor());
    assert_eq!("draft to keep", app.pane().draft.text());
    assert_eq!(
        Some("low"),
        app.pane().effective_model_identity().unwrap().2
    );
}

#[test]
fn thinking_cycle_keeps_the_draft_route_and_other_cells_stable_until_it_resolves() {
    for width in [60, 120] {
        let workspace = std::path::PathBuf::from("/workspace");
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
        let version = app
            .effects
            .iter()
            .find_map(|effect| match effect {
                Job::Model(_, version) => Some(*version),
                _ => None,
            })
            .expect("initial model request");
        let model: kuku_client::ModelSelection = serde_json::from_value(json!({
            "connection": "fixture",
            "protocol": "openai-responses",
            "model": "gemini-4-neon",
            "context_window_tokens": 100_000,
            "settings": {"reasoning_effort": "low"},
            "capabilities": {"reasoning_efforts": ["low", "medium", "high"]}
        }))
        .unwrap();
        app.update(Update::DefaultModel(version, Ok(model.clone())));
        app.effects.clear();
        app.paste("keep this draft");
        let before = frame(&mut app, width);
        let cycle = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
        app.key(cycle);
        app.key(cycle);
        let during = frame(&mut app, width);
        assert_eq!(before, during);
        let [Job::Model(config, version)] = app.effects.as_slice() else {
            panic!("Expected one in-flight model preview");
        };
        assert_eq!(config.reasoning_effort.as_deref(), Some("medium"));
        let mut model = model;
        model.settings.reasoning_effort = Some("medium".into());
        app.update(Update::DefaultModel(*version, Ok(model)));
        let after = frame(&mut app, width);
        let (x, y) = position(&before, "low");
        assert_eq!((x, y), position(&after, "medium"));
        assert_eq!(
            position(&before, "Ctrl+T think"),
            position(&after, "Ctrl+T think")
        );
        for (index, (previous, current)) in before.content.iter().zip(&after.content).enumerate() {
            let diff_x = (index % usize::from(width)) as u16;
            let diff_y = (index / usize::from(width)) as u16;
            if diff_y != y || !(x..x + 7).contains(&diff_x) {
                assert_eq!(previous, current, "changed cell at {diff_x},{diff_y}");
            }
        }
    }
}

#[test]
fn use_default_clears_a_legacy_override_without_reconfiguring_the_route() {
    let mut app = connected_app();
    load_model(&mut app, Some("provider-default"), None);
    app.open_thinking();
    let screen = frame(&mut app, 60);
    let text = lines(&screen).join("\n");
    assert!(text.contains("gemini-4-neon Default"), "{text}");
    assert!(text.contains("Current · Default"), "{text}");
    assert!(!text.contains("Provider default"), "{text}");
    app.effects.clear();
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.effects.as_slice(), [Job::Save(command), Job::ClosePicker]
        if matches!(&command.action, Action::Configure { config }
            if config.reasoning_effort.is_none() && config.tier.as_deref() == Some("fixture-model")))
    );
}

#[test]
fn speed_uses_the_composer_and_applies_declared_tiers_without_probing() {
    let mut app = connected_app();
    load_model(&mut app, Some("high"), None);

    action_picker(&mut app, "speed");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if picker.kind == PickerKind::Speed && picker.kind.composer_anchored()));
    assert!(app
        .effects
        .iter()
        .all(|job| !matches!(job, Job::CustomSpeed(..))));

    app.paste("provider-custom");
    press(&mut app, KeyCode::Enter);
    assert!(app
        .effects
        .iter()
        .any(|job| matches!(job, Job::Save(command)
        if command.target.agent == "main"
        && matches!(&command.action, Action::Configure { config }
            if config.service_tier.as_deref() == Some("provider-custom")
            && config.reasoning_effort.as_deref() == Some("high")))));
}

#[test]
fn custom_speed_can_be_saved_without_probing_and_then_applied() {
    use kuku::config::CustomServiceTier;

    use crate::setup::service_tier::{SpeedResult, SpeedWork};

    let mut app = connected_app();
    load_model(&mut app, Some("high"), None);
    action_picker(&mut app, "speed turbo");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker))
        if picker.kind == PickerKind::CustomSpeed && picker.kind.composer_anchored()));
    assert!(app
        .effects
        .iter()
        .all(|job| !matches!(job, Job::CustomSpeed(..))));

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    let (ticket, target) = std::mem::take(&mut app.effects)
        .into_iter()
        .find_map(|job| match job {
            Job::CustomSpeed(
                ticket,
                target,
                SpeedWork::Save(tier, CustomServiceTier::Unverified),
            ) if tier == "turbo" => Some((ticket, target)),
            _ => None,
        })
        .expect("save without a probe");
    app.update(Update::SpeedFinished(
        ticket,
        target,
        Ok(SpeedResult::Saved("turbo".into())),
    ));
    assert!(app.effects.iter().any(|job| matches!(job, Job::Save(command)
        if matches!(&command.action, Action::Configure { config } if config.service_tier.as_deref() == Some("turbo")))));
}

#[test]
fn model_picker_letters_are_actions_until_search_is_focused() {
    use crate::menu::{Choice, ChoiceKind};

    let mut app = connected_app();
    let tier =
        serde_json::from_value(json!({"name":"deep", "connection":"gateway", "model":"fixture"}))
            .unwrap();
    app.picker(
        PickerKind::Models,
        vec![Choice::new(
            "deep",
            "",
            ChoiceKind::ModelRoute {
                tier: Box::new(tier),
                current: false,
            },
        )],
    );

    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('e'));
    assert!(matches!(&app.modal, Some(Modal::Picker(picker)) if picker.query.text() == "de"));
    assert!(app.effects.is_empty());
    press(&mut app, KeyCode::Esc);
    assert!(matches!(&app.modal, Some(Modal::Picker(picker)) if !picker.search_focused));

    press(&mut app, KeyCode::Char('d'));
    let (selection, tier) = std::mem::take(&mut app.effects)
        .into_iter()
        .find_map(|job| match job {
            Job::SaveAndUseModel(selection, tier) => Some((selection, tier)),
            _ => None,
        })
        .expect("queue the default save directly");
    app.update(Update::ModelDefaultAndSwitchSaved(
        selection.clone(),
        tier.clone(),
        Err("Save failed".into()),
    ));
    assert!(app.pane().config().tier.is_none());
    app.update(Update::ModelDefaultAndSwitchSaved(
        selection,
        tier,
        Ok(true),
    ));
    assert_eq!(Some("deep"), app.pane().config().tier.as_deref());
}

#[test]
fn model_picker_aligns_details_and_fits_the_account_and_route_markers() {
    use crate::menu::{Choice, ChoiceKind};

    let mut app = connected_app();
    let route = |name, detail| {
        let tier =
            serde_json::from_value(json!({"name":name, "connection":"fixture", "model":name}))
                .unwrap();
        Choice::new(
            name,
            detail,
            ChoiceKind::ModelRoute {
                tier: Box::new(tier),
                current: false,
            },
        )
    };
    app.picker(
        PickerKind::Models,
        vec![
            route(
                "gpt-6.2-sol",
                "ChatGPT · team-operations · ali***e@exa***.com · Current · Default",
            ),
            route("gpt-6.2-astra", "ChatGPT · other · sam***e@exa***.com"),
            Choice::new("Set current model as default", "", ChoiceKind::Close),
        ],
    );
    let screen = frame(&mut app, 94);
    assert_eq!(
        position(&screen, "ChatGPT · team-operations").0,
        position(&screen, "ChatGPT · other").0
    );
    position(&screen, "ali***e@exa***.com · Current · Default");
}
