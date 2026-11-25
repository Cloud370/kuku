use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kuku::config::{ModelRef, ThinkLevel};
use kuku::session::AgentConfig;
use kuku_client::local::ServiceIdentity;
use kuku_client::{Connection, ConnectionOptions};
use ratatui::{Frame, layout::Rect};

use crate::editor::Editor;
use crate::local_settings::{active_service, ensure_same_service, wait_for_config};

use super::config::{self, Settings};
use super::state::{SetupIntent, SetupState};
use super::{Finish, SetupMode};

const MAX_MODEL_BYTES: usize = 256;
const MAX_KEY_BYTES: usize = 4096;

pub(crate) struct ModelEditor {
    settings: Settings,
    state: SetupState,
    service: Option<ServiceIdentity>,
    usable: bool,
    exit_after_save: bool,
}

pub(crate) enum ModelEditorAction {
    None,
    Cancel,
    Save,
}

#[derive(Debug, Clone)]
pub(crate) struct ThinkingSetting {
    pub(crate) target: ThinkingSaveTarget,
    pub(crate) effort: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ThinkingSaveTarget {
    route: ModelRef,
    service: ServiceIdentity,
}

pub(crate) struct ThinkingDefaults {
    pub(crate) inherited: Option<kuku::ProviderSettings>,
    pub(crate) save: Option<ThinkingSaveTarget>,
}

impl ModelEditor {
    pub(crate) async fn open(
        options: &ConnectionOptions,
        connection: Option<&Connection>,
        intent: SetupIntent,
    ) -> Result<Self, String> {
        let (service, settings) = if connection.is_some() {
            let (service, settings) = active_settings(options, connection).await?;
            (Some(service), settings)
        } else {
            let settings = Settings::load_for_first_run(options.clone())
                .await
                .map_err(|error| error.to_string())?
                .ok_or("Local service or configuration changed. Use Ctrl+R to reconnect.")?;
            (None, settings)
        };
        if matches!(&intent, SetupIntent::Edit(route) if settings.model(route).is_none()) {
            return Err("The configured model changed. Reopen /model.".into());
        }
        let state = SetupState::for_intent(&settings, intent);
        Ok(Self {
            settings,
            state,
            service,
            usable: false,
            exit_after_save: false,
        })
    }

    pub(crate) fn is_first_run(&self) -> bool {
        self.service.is_none()
    }

    pub(crate) fn exit_after_save(&self) -> bool {
        self.exit_after_save
    }

    fn mode(&self) -> SetupMode {
        if self.is_first_run() {
            SetupMode::FirstRun
        } else {
            SetupMode::Session
        }
    }

    pub(crate) fn handle(&mut self, event: Event) -> ModelEditorAction {
        let mode = self.mode();
        if matches!(
            event,
            Event::Key(KeyEvent {
                code: KeyCode::F(1),
                kind: KeyEventKind::Press,
                ..
            })
        ) && mode == SetupMode::FirstRun
        {
            self.state.problem = configuration_guide_url()
                .ok_or_else(|| {
                    "This build has no source revision for the configuration guide.".to_owned()
                })
                .and_then(|url| crate::open_browser(&url).map_err(|error| error.to_string()))
                .err();
            return ModelEditorAction::None;
        }
        if let Event::Resize(width, height) = event {
            self.usable = super::ui::usable(width, height);
        }
        if self.usable {
            self.state.handle(event, &self.settings, mode);
        } else {
            super::ui::handle_small_terminal_event(&mut self.state, &event);
        }
        match self.state.finish.take() {
            Some(Finish::Cancel) => ModelEditorAction::Cancel,
            Some(finish @ (Finish::SaveAndContinue | Finish::SaveAndExit)) => {
                self.exit_after_save = finish == Finish::SaveAndExit;
                ModelEditorAction::Save
            }
            None => ModelEditorAction::None,
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>) {
        let mode = self.mode();
        let area = frame.area();
        self.usable = super::ui::usable(area.width, area.height);
        super::ui::render(frame, &self.settings, &mut self.state, mode);
    }

    pub(crate) fn authorization_link(
        &self,
        area: Rect,
    ) -> Option<crate::terminal::AuthorizationLink> {
        super::ui::authorization_link(&self.state, area)
    }

    pub(crate) fn probe_deadline(&self) -> Option<std::time::Instant> {
        self.state
            .probe_deadline()
            .into_iter()
            .chain(self.state.subscription_deadline())
            .min()
    }

    pub(crate) fn poll_probes(&mut self) -> bool {
        let mode = self.mode();
        let registration = self.state.poll_subscription(&mut self.settings);
        self.state.poll_probes(&self.settings, mode) || registration
    }

    pub(crate) fn failed(&mut self, problem: String) {
        self.state.problem = Some(problem);
    }

    pub(crate) async fn save(
        &self,
        options: &ConnectionOptions,
        connection: Option<&Connection>,
    ) -> Result<bool, String> {
        let Some(service) = &self.service else {
            if !super::first_run_service_is_idle(options).await {
                return Err("Local service changed. Use Ctrl+R to reconnect.".into());
            }
            config::save(&self.settings, &self.state).map_err(|error| error.to_string())?;
            return Ok(true);
        };
        let current = active_service(options, connection).await?;
        ensure_same_service(service, &current)?;
        let (_, hash) =
            config::save(&self.settings, &self.state).map_err(|error| error.to_string())?;
        Ok(wait_for_config(connection, service, &hash).await)
    }
}

pub(crate) async fn thinking_defaults(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    mut config: AgentConfig,
    route: ModelRef,
) -> ThinkingDefaults {
    config.reasoning_effort = None;
    let inherited = async {
        let resolved = connection?.client.resolve_model(&config).await.ok()?;
        (resolved.connection == route.connection && resolved.model == route.model)
            .then_some(resolved.settings)
    };
    let save = async {
        let (service, settings) = active_settings(options, connection).await.ok()?;
        settings.model(&route)?;
        Some(ThinkingSaveTarget {
            route: route.clone(),
            service,
        })
    };
    let (inherited, save) = tokio::join!(inherited, save);
    ThinkingDefaults { inherited, save }
}

pub(crate) async fn save_setting(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    change: ThinkingSetting,
) -> Result<bool, String> {
    let (service, settings) = active_settings(options, connection).await?;
    let current = active_service(options, connection).await?;
    ensure_same_service(&service, &current)?;
    ensure_same_service(&change.target.service, &current)?;
    let route = &change.target.route;
    if settings.model(route).is_none() {
        return Err(
            "Choose a configured model in /model before saving its thinking setting.".into(),
        );
    }
    let mut document = settings.document.clone();
    let key = config::model_config_key(&document, route, &|name| std::env::var(name));
    let fields = config::model_entry(&mut document, &key).map_err(|error| error.to_string())?;
    fields.insert(
        "think",
        toml_edit::value(ThinkLevel::ProviderDefault.as_str()),
    );
    fields.insert("reasoning_effort", toml_edit::value(change.effort));
    let hash = config::save_model_document(&settings, &document, &[route])?;
    Ok(wait_for_config(connection, &service, &hash).await)
}

pub(super) async fn active_settings(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
) -> Result<(ServiceIdentity, Settings), String> {
    let service = active_service(options, connection).await?;
    let settings = Settings::load_with_status(
        options.clone(),
        Some(service.config_path.clone()),
        &|name| std::env::var(name),
    )
    .map_err(|error| error.to_string())?;
    Ok((service, settings))
}

fn configuration_guide_url() -> Option<String> {
    let revision = kuku::BUILD_REVISION;
    (!revision.is_empty()).then(|| {
        format!("https://github.com/Cloud370/kuku/blob/{revision}/docs/en/start/configuration.md")
    })
}

pub(super) fn valid_model_id(input: &str) -> Result<String, String> {
    let model = input.trim();
    if model.is_empty() {
        return Err("Enter a model ID.".into());
    }
    if model.len() > MAX_MODEL_BYTES
        || model
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("Model IDs must be a single line of at most 256 bytes.".into());
    }
    if model.starts_with('$') {
        return Err("Enter the exact model ID. Use config.toml for environment references.".into());
    }
    Ok(model.to_owned())
}

pub(super) fn valid_env_name(input: &str) -> Result<String, String> {
    let name = input.trim();
    let mut characters = name.chars();
    if !characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err("Enter your environment-variable name, for example MY_GATEWAY_KEY.".into());
    }
    Ok(name.to_owned())
}

pub(super) fn valid_stored_key(input: &str) -> Result<String, String> {
    if input.chars().any(char::is_control) {
        return Err("Keys must be a single line of at most 4096 bytes.".into());
    }
    let key = input.trim();
    if key.is_empty() {
        return Err("Enter a key or leave blank to keep the saved key.".into());
    }
    if key.len() > MAX_KEY_BYTES {
        return Err("Keys must be a single line of at most 4096 bytes.".into());
    }
    Ok(key.to_owned())
}

pub(super) fn edit_field(editor: &mut Editor, key: KeyEvent) {
    match key.code {
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Delete => editor.delete(),
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            editor.insert(&character.to_string());
        }
        _ => {}
    }
}
