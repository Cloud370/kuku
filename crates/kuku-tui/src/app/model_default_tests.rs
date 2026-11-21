use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use kuku_client::{
    Action, AgentConfig, AgentSelection, AgentSnapshot, AgentStatus, AgentSummary, ApiError,
    Command, ModelSelection, Snapshot, UsageTotals,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{App, Modal, PendingDefaultSave, PendingStage};
use crate::jobs::Job;
use crate::menu::PickerKind;
use crate::model_default::ResolvedModel;

fn model() -> ModelSelection {
    ModelSelection {
        connection: "openai".into(),
        protocol: kuku::config::ProviderFormat::OpenAiResponses,
        model: "balanced-model".into(),
        context_window_tokens: 0,
        max_output_tokens: None,
        settings: Default::default(),
        output: Default::default(),
        request_url: "https://example.test/v1/messages".into(),
        capabilities: Default::default(),
    }
}

fn selection(connection: &str, model: &str) -> AgentSelection {
    AgentSelection {
        connection: connection.into(),
        protocol: kuku::config::ProviderFormat::OpenAiResponses,
        model: model.into(),
        context_window_tokens: 16_000,
        max_output_tokens: Some(1_000),
        settings: Default::default(),
        output: Default::default(),
        request_url: "https://example.test/v1/messages".into(),
        capabilities: Default::default(),
        tools: Vec::new(),
    }
}

fn snapshot(selection: AgentSelection, pending_selection: Option<AgentSelection>) -> Snapshot {
    let config = AgentConfig::default();
    let agent = AgentSnapshot {
        address: "main".into(),
        config: config.clone(),
        active_config: Some(config),
        status: AgentStatus::Running,
        pending_inputs: 0,
        admitted_task: Some("unchanged task".into()),
        queued_task: None,
        turn: Some(3),
        turn_started_at_ms: Some(1),
        turn_elapsed_ms: None,
        execution_id: Some(12),
        last_output: None,
        last_error: None,
        permission: None,
        question: None,
        usage: UsageTotals::default(),
    };
    let summary = AgentSummary {
        address: "main".into(),
        status: AgentStatus::Running,
        turn: Some(3),
        execution_id: Some(12),
        pending_inputs: 0,
        admitted_task: Some("unchanged task".into()),
        queued_task: None,
        question: None,
        permission: None,
        last_output_preview: None,
        last_error_preview: None,
        progress: None,
    };
    Snapshot {
        session_id: "session".into(),
        workspace: PathBuf::from("workspace"),
        cursor: 31,
        agent,
        agents: vec![summary],
        pending: Vec::new(),
        selection: Some(selection),
        selection_error: None,
        pending_selection,
        pending_selection_error: None,
        cleaning_up: false,
        progress: None,
        reply_preview: None,
    }
}

#[test]
fn stale_preparation_cannot_reopen_confirmation_for_another_target() {
    let mut app = App::new(PathBuf::new(), None);
    let picker = app.picker(PickerKind::ModelDefault, Vec::new());
    let ticket = picker.ticket.id;
    let old_selection = app.selection();
    app.generation = app.generation.wrapping_add(1);
    app.prepared_model_default(ticket, old_selection, Err("ignored".into()));
    assert!(app.modal.is_none());
    assert!(!app
        .effects
        .iter()
        .any(|job| matches!(job, Job::SaveDefault { .. })));
}

#[test]
fn confirmed_default_save_queues_file_persistence_not_agent_configuration() {
    let mut app = App::new(PathBuf::new(), None);
    app.pane_mut().initial_model = Some(model());
    let route = app.resolved_model_route().unwrap();
    let ticket = 41;
    app.pending_model_default = Some(PendingDefaultSave {
        ticket,
        selection: app.selection(),
        plan: crate::model_default::test_plan(route),
    });
    app.confirm_model_default(ticket);
    assert!(matches!(app.effects.last(), Some(Job::SaveDefault { .. })));
    assert!(!app
        .effects
        .iter()
        .any(|job| matches!(job, Job::Save(_) | Job::Send(_))));
}

#[test]
fn resolved_pending_selection_is_saved_without_changing_current_execution() {
    let mut app = App::new(PathBuf::new(), None);
    let current = selection("openai", "model-a");
    let chosen = selection("anthropic", "model-b");
    let snapshot = snapshot(current, Some(chosen.clone()));
    app.pane_mut().exists = true;
    app.pane_mut().snapshot = Some(Box::new(snapshot.clone()));
    let initial_config = app.pane().initial_config.clone();

    let route = app.resolved_model_route().unwrap();
    assert_eq!("anthropic", route.connection);
    assert_eq!("model-b", route.model);

    let picker = app.picker(PickerKind::ModelDefault, Vec::new());
    let ticket = picker.ticket.id;
    let active_selection = app.selection();
    app.prepared_model_default(
        ticket,
        active_selection,
        Ok(crate::model_default::test_plan(ResolvedModel {
            connection: "anthropic".into(),
            model: "model-b".into(),
        })),
    );
    let Some(Modal::Picker(picker)) = &app.modal else {
        panic!("prepared default must open its confirmation");
    };
    assert!(picker
        .header
        .contains("Connection/model: anthropic/model-b"));
    assert!(!picker.header.contains("Connection/model: openai/model-a"));
    app.confirm_model_default(ticket);
    let Some(Job::SaveDefault { plan, .. }) = app.effects.last() else {
        panic!("default save should be queued");
    };
    assert_eq!("anthropic", plan.route().connection);
    assert_eq!("model-b", plan.route().model);
    assert_eq!(initial_config, app.pane().initial_config);
    assert_eq!(Some(&snapshot), app.pane().snapshot.as_deref());
    assert_eq!(
        Some(12),
        app.pane().snapshot.as_ref().unwrap().agent.execution_id
    );
}

#[test]
fn pending_configuration_and_receipt_block_default_route() {
    let mut app = App::new(PathBuf::new(), None);
    let snapshot = snapshot(selection("openai", "model-a"), None);
    app.pane_mut().exists = true;
    app.pane_mut().snapshot = Some(Box::new(snapshot));
    let target = app.target(&app.active);
    let command = Command {
        target,
        command_id: "configure-1".into(),
        action: Action::Configure {
            config: Box::new(AgentConfig::default()),
        },
    };
    app.pending.insert(
        command.command_id.clone(),
        crate::app::PendingItem {
            command,
            stage: PendingStage::Saving,
            origin: None,
        },
    );
    assert!(app.resolved_model_route().is_err());
    app.pending.clear();
    app.pane_mut().config_receipt = Some((2, Box::new(AgentConfig::default())));
    assert!(app.pane().waiting_for_configuration());
    assert!(app.resolved_model_route().is_err());
}

#[test]
fn failed_pending_selection_and_draft_resolution_are_rejected() {
    let mut app = App::new(PathBuf::new(), None);
    let mut current = snapshot(selection("openai", "model-a"), None);
    current.pending_selection_error = Some(ApiError {
        code: "invalid_selection".into(),
        message: "unavailable".into(),
    });
    app.pane_mut().exists = true;
    app.pane_mut().snapshot = Some(Box::new(current));
    assert!(app.resolved_model_route().is_err());

    let mut draft = App::new(PathBuf::new(), None);
    draft.pane_mut().initial_model = Some(model());
    draft.pane_mut().model_problem = Some("preview failed".into());
    assert!(draft.resolved_model_route().is_err());
}

fn saved_update_keeps_current_agent_state(result: Result<(), String>) {
    let mut app = App::new(PathBuf::new(), None);
    let snapshot = snapshot(selection("openai", "model-a"), None);
    app.pane_mut().exists = true;
    app.pane_mut().initial_config.instructions = Some("unchanged config".into());
    app.pane_mut().snapshot = Some(Box::new(snapshot));
    let before_config = app.pane().initial_config.clone();
    let before_snapshot = app.pane().snapshot.clone();
    let ticket = 73;
    let selection = app.selection();
    app.saving_model_default = Some((ticket, selection.clone()));
    app.saved_model_default(ticket, selection, result);
    assert_eq!(before_config, app.pane().initial_config);
    assert_eq!(before_snapshot, app.pane().snapshot);
}

#[test]
fn successful_default_save_update_keeps_current_agent_state() {
    saved_update_keeps_current_agent_state(Ok(()));
}

#[test]
fn failed_default_save_update_keeps_current_agent_state() {
    saved_update_keeps_current_agent_state(Err("save failed".into()));
}

#[test]
fn escape_closes_prepared_default_confirmation_without_queuing_a_write() {
    let mut app = App::new(PathBuf::new(), None);
    app.pane_mut().initial_model = Some(model());
    let picker = app.picker(PickerKind::ModelDefault, Vec::new());
    let ticket = picker.ticket.id;
    let selected = app.selection();
    let route = app.resolved_model_route().unwrap();
    app.prepared_model_default(
        ticket,
        selected.clone(),
        Ok(crate::model_default::test_plan(route)),
    );
    assert!(
        matches!(&app.modal, Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::ModelDefault))
    );
    assert!(app.pending_model_default.is_some());
    let Some(Modal::Picker(picker)) = &app.modal else {
        panic!("confirmation must be open before Escape");
    };
    assert_eq!(1, picker.selected);
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &Buffer::empty(Rect::new(0, 0, 60, 20)),
    );
    assert!(app.modal.is_none());
    assert!(app.pending_model_default.is_none());
    assert!(!app
        .effects
        .iter()
        .any(|job| matches!(job, Job::SaveDefault { .. })));
}
