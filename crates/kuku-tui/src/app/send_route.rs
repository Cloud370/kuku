use kuku::session::runtime::ModelRequestRoute;
use kuku_client::{Action, AgentConfig, Timing};

use crate::jobs::{Job, Selection};

use super::{App, Modal, PendingStage};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RouteChange {
    previous: ModelRequestRoute,
    next: ModelRequestRoute,
    tier_changed: bool,
}

#[derive(Debug)]
pub(crate) struct SendRouteConfirmation {
    ticket: u64,
    selection: Selection,
    draft_revision: u64,
    timing: Timing,
    change: RouteChange,
    labels: [Option<String>; 2],
}

impl SendRouteConfirmation {
    pub(crate) fn title(&self) -> &'static str {
        if self.change.previous.same_route(&self.change.next) {
            "Service tier changed · Cache may miss"
        } else if self.change.previous.model == self.change.next.model
            && self.change.previous.connection == self.change.next.connection
        {
            "Model endpoint changed · Cache may miss"
        } else {
            "Model route changed · Cache may miss"
        }
    }

    pub(crate) fn change_labels(&self) -> [String; 2] {
        let routes = [&self.change.previous, &self.change.next];
        let tier_label = |index: usize| {
            routes[index]
                .service_tier()
                .flatten()
                .unwrap_or("Provider default")
        };
        if routes[0].same_route(routes[1]) {
            return std::array::from_fn(|index| tier_label(index).to_owned());
        }
        let show_connection = routes[0].connection != routes[1].connection;
        std::array::from_fn(|index| {
            let model = &routes[index].model;
            let model = if self.change.tier_changed {
                format!("{model} {}", tier_label(index))
            } else {
                model.clone()
            };
            if show_connection {
                let label = self.labels[index]
                    .as_deref()
                    .unwrap_or(["Previous connection", "New connection"][index]);
                format!("{model} · {label}")
            } else {
                model
            }
        })
    }
}

impl App {
    pub(super) fn send_route_change(&self) -> Option<RouteChange> {
        let snapshot = self.pane().snapshot.as_ref()?;
        let previous = snapshot.agent.last_model_route.clone()?;
        if snapshot.pending_selection_error.is_some() {
            return None;
        }
        let selection = snapshot
            .pending_selection
            .as_ref()
            .or(snapshot.selection.as_ref())?;
        let next = ModelRequestRoute::from_selection(selection)?;
        let tier_changed = match (previous.service_tier(), next.service_tier()) {
            (Some(Some(previous)), Some(Some(next))) => !selection
                .capabilities
                .equivalent_service_tiers(previous, next),
            (Some(previous), Some(next)) => previous != next,
            _ => false,
        };
        (!previous.same_route(&next) || tier_changed).then_some(RouteChange {
            previous,
            next,
            tier_changed,
        })
    }

    pub(super) fn send_route_is_updating(&self) -> bool {
        let Some(snapshot) = &self.pane().snapshot else {
            return false;
        };
        if snapshot.agent.last_model_route.is_none() {
            return false;
        }
        let settings_changed = |config: &AgentConfig| {
            let observed = &snapshot.agent.config;
            config.tier != observed.tier
                || config.connection != observed.connection
                || config.model != observed.model
                || config.service_tier != observed.service_tier
        };
        let target = self.target(&self.active);
        settings_changed(self.pane().config())
            || self.pending.values().any(|item| {
                item.command.target == target && !matches!(item.stage, PendingStage::Rejected(_))
                    && matches!(&item.command.action, Action::Configure { config } if settings_changed(config))
            })
    }

    pub(super) fn review_send_route(&mut self, timing: Timing, change: RouteChange) {
        let ticket = self.modal_id();
        if change.previous.connection != change.next.connection {
            self.effects.push(Job::SendRouteLabels(
                ticket,
                [
                    change.previous.connection.clone(),
                    change.next.connection.clone(),
                ],
            ));
        }
        self.modal = Some(Modal::SendRouteConfirmation(Box::new(
            SendRouteConfirmation {
                ticket,
                selection: self.selection(),
                draft_revision: self.pane().draft_revision,
                timing,
                change,
                labels: [None, None],
            },
        )));
    }

    pub(crate) fn confirm_send_route(&mut self) {
        let Some(Modal::SendRouteConfirmation(review)) = self.modal.take() else {
            return;
        };
        self.close_picker();
        if review.selection == self.selection()
            && review.draft_revision == self.pane().draft_revision
        {
            self.send_with_route(review.timing, Some(&review.change));
        }
    }

    pub(super) fn send_route_labels(&mut self, ticket: u64, labels: [Option<String>; 2]) {
        if let Some(Modal::SendRouteConfirmation(review)) = &mut self.modal {
            if review.ticket == ticket {
                review.labels = labels;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use kuku_client::{AgentSelection, HistoryPage, Snapshot};
    use ratatui::buffer::Buffer;
    use serde_json::json;

    use crate::jobs::Update;

    use super::{Action, App, Job, Modal, ModelRequestRoute, Timing};

    fn model(connection: &str, id: &str) -> AgentSelection {
        serde_json::from_value(json!({
            "connection": connection, "model": id, "protocol": "openai-responses",
            "request_url": "http://fixture/v1/responses", "context_window_tokens": 96_000,
            "settings": {}, "capabilities": {}, "tools": [],
        }))
        .unwrap()
    }

    fn ready(previous: &AgentSelection, selection: AgentSelection) -> App {
        let workspace = PathBuf::from("/workspace");
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
        let snapshot: Snapshot = serde_json::from_value(json!({
            "session_id": app.active.session, "workspace": app.workspace, "cursor": 8,
            "agent": {"address":"main", "status":"idle", "config":{}, "pending_inputs":0,
                "last_model_route": ModelRequestRoute::from_selection(previous)},
            "pending": [], "agents": [], "selection": selection
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
        app.pane_mut().draft.insert("Keep this draft");
        app.effects.clear();
        app
    }

    fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) {
        app.event(
            Event::Key(KeyEvent::new_with_kind(code, modifiers, kind)),
            &Buffer::empty(Default::default()),
        );
    }

    fn sent(app: &App) -> Vec<Timing> {
        app.effects
            .iter()
            .filter_map(|job| match job {
                Job::Save(command) => match &command.action {
                    Action::Send { timing, .. } => Some(*timing),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn route_change_requires_a_new_keypress_and_keeps_next_turn_timing() {
        let mut app = ready(&model("gateway", "first"), model("gateway", "second"));
        key(
            &mut app,
            KeyCode::Enter,
            KeyModifiers::ALT,
            KeyEventKind::Press,
        );
        let Some(Modal::SendRouteConfirmation(review)) = &app.modal else {
            panic!("confirmation expected")
        };
        assert_eq!(review.change_labels(), ["first", "second"]);
        key(
            &mut app,
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        );
        assert!(sent(&app).is_empty());
        assert_eq!(app.pane().draft.text(), "Keep this draft");
        key(
            &mut app,
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        );
        assert_eq!(sent(&app), [Timing::NextTurn]);
    }

    #[test]
    fn escaping_or_editing_a_route_review_keeps_the_draft() {
        let mut app = ready(&model("old", "same"), model("new", "same"));
        app.send(Timing::Steer);
        let Some(Modal::SendRouteConfirmation(review)) = &app.modal else {
            panic!("confirmation expected")
        };
        assert_eq!(
            review.change_labels(),
            ["same · Previous connection", "same · New connection"]
        );
        key(
            &mut app,
            KeyCode::Esc,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        );
        assert!(app.modal.is_none());
        assert_eq!(app.pane().draft.text(), "Keep this draft");
        app.send(Timing::Steer);
        key(
            &mut app,
            KeyCode::Char('!'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        );
        assert!(app.modal.is_none());
        assert_eq!(app.pane().draft.text(), "Keep this draft!");
        key(
            &mut app,
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        );
        assert!(matches!(app.modal, Some(Modal::SendRouteConfirmation(_))));
        assert!(sent(&app).is_empty());
    }

    #[test]
    fn current_selection_is_checked_again_before_sending() {
        let mut app = ready(&model("gateway", "first"), model("gateway", "second"));
        app.send(Timing::Steer);
        let mut snapshot = app.pane().snapshot.clone().unwrap();
        snapshot.cursor += 1;
        snapshot.selection = Some(model("gateway", "third"));
        app.update(Update::Refreshed(app.selection(), Ok(snapshot)));
        key(
            &mut app,
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        );
        let Some(Modal::SendRouteConfirmation(review)) = &app.modal else {
            panic!("new confirmation expected")
        };
        assert_eq!(review.change_labels(), ["first", "third"]);
        assert!(sent(&app).is_empty());
    }

    #[test]
    fn changed_reasoning_and_equivalent_service_tiers_send_directly() {
        let mut previous = model("gateway", "same");
        previous.settings.service_tier = Some("priority".into());
        let mut next = previous.clone();
        next.capabilities.profile = kuku::config::ProviderProfile::OpenAi;
        next.settings.reasoning_effort = Some("high".into());
        next.settings.service_tier = Some("fast".into());
        let mut app = ready(&previous, next);
        app.send(Timing::Steer);
        assert!(app.modal.is_none());
        assert_eq!(sent(&app), [Timing::Steer]);
    }

    #[test]
    fn a_changed_service_tier_prompts_on_send_and_shows_the_previous_tier() {
        let previous = model("gateway", "same");
        let mut next = previous.clone();
        next.settings.service_tier = Some("ultrafast".into());
        let mut app = ready(&previous, next);
        app.send(Timing::Steer);
        let Some(Modal::SendRouteConfirmation(review)) = &app.modal else {
            panic!("service tier confirmation expected")
        };
        assert_eq!(review.title(), "Service tier changed · Cache may miss");
        assert_eq!(review.change_labels(), ["Provider default", "ultrafast"]);
        assert!(sent(&app).is_empty());
        key(
            &mut app,
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        );
        assert_eq!(sent(&app), [Timing::Steer]);
    }

    #[test]
    fn unknown_previous_service_tier_sends_directly() {
        let previous = model("gateway", "same");
        let mut next = previous.clone();
        next.settings.service_tier = Some("ultrafast".into());
        let mut app = ready(&previous, next);
        let mut snapshot = app.pane().snapshot.clone().unwrap();
        snapshot.cursor += 1;
        let mut previous = serde_json::to_value(snapshot.agent.last_model_route).unwrap();
        previous.as_object_mut().unwrap().remove("service_tier");
        snapshot.agent.last_model_route = serde_json::from_value(previous).unwrap();
        app.update(Update::Refreshed(app.selection(), Ok(snapshot)));
        app.send(Timing::Steer);
        assert!(app.modal.is_none());
        assert_eq!(sent(&app), [Timing::Steer]);
    }
}
