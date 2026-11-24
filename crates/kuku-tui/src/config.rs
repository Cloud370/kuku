use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use kuku_client::ConnectionOptions;
use serde::Deserialize;
use tempfile::NamedTempFile;
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::notifications::Notifications;
use crate::statusline::StatusLine;

const FILE_NAME: &str = "tui.toml";
const STATUSLINE_ENV: &str = "KUKU_TUI_STATUSLINE";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TuiFile {
    statusline: Option<StatusLineFile>,
    notifications: Option<Notifications>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusLineFile {
    fields: Option<Vec<String>>,
}

#[derive(Debug)]
pub(crate) struct NotificationSettings {
    path: Result<PathBuf, String>,
    active: Notifications,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            path: Err("The client data directory is unavailable".into()),
            active: Notifications::default(),
        }
    }
}

impl NotificationSettings {
    pub(crate) fn active(&self) -> Notifications {
        self.active
    }

    pub(crate) fn save(&mut self, settings: Notifications) -> Result<(), String> {
        let path = self.path.as_deref().map_err(Clone::clone)?;
        save_notifications(path, settings).map_err(|error| file_problem(path, error))?;
        self.active = settings;
        Ok(())
    }
}

pub(crate) fn load_notifications(
    options: &ConnectionOptions,
) -> (NotificationSettings, Option<String>) {
    let mut settings = NotificationSettings {
        path: preferences_path(options),
        ..NotificationSettings::default()
    };
    let result = settings
        .path
        .as_deref()
        .map_err(Clone::clone)
        .and_then(|path| {
            let content = read_file(path).map_err(|error| file_problem(path, error))?;
            let file: TuiFile =
                toml::from_str(&content).map_err(|error| file_problem(path, error))?;
            Ok(file.notifications.unwrap_or_default())
        });
    match result {
        Ok(active) => {
            settings.active = active;
            (settings, None)
        }
        Err(error) => (settings, Some(error)),
    }
}

fn preferences_path(options: &ConnectionOptions) -> Result<PathBuf, String> {
    options
        .data_dir()
        .map(|path| path.join(FILE_NAME))
        .map_err(|error| format!("Locate {FILE_NAME} · {error}"))
}

#[derive(Debug)]
pub(crate) struct StatusLineSettings {
    path: Result<PathBuf, String>,
    saved: StatusLine,
    override_line: Option<StatusLine>,
}

impl StatusLineSettings {
    pub(crate) fn active(&self) -> &StatusLine {
        self.override_line.as_ref().unwrap_or(&self.saved)
    }

    pub(crate) fn overridden(&self) -> bool {
        self.override_line.is_some()
    }

    pub(crate) fn read(&self) -> Result<StatusLine, String> {
        let path = self.path.as_deref().map_err(Clone::clone)?;
        let content = read_file(path).map_err(|error| file_problem(path, error))?;
        parse_file(&content).map_err(|error| file_problem(path, error))
    }

    pub(crate) fn save(&mut self, layout: StatusLine) -> Result<(), String> {
        let path = self.path.as_deref().map_err(Clone::clone)?;
        save_statusline(path, &layout).map_err(|error| file_problem(path, error))?;
        self.saved = layout;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn in_memory(layout: StatusLine) -> Self {
        Self {
            saved: layout,
            ..Self::default()
        }
    }
}

impl Default for StatusLineSettings {
    fn default() -> Self {
        Self {
            path: Err("The client data directory is unavailable".into()),
            saved: StatusLine::default(),
            override_line: None,
        }
    }
}

pub(crate) fn load_statusline(options: &ConnectionOptions) -> (StatusLineSettings, Option<String>) {
    resolve_statusline(options, env::var_os(STATUSLINE_ENV))
}

fn resolve_statusline(
    options: &ConnectionOptions,
    override_value: Option<OsString>,
) -> (StatusLineSettings, Option<String>) {
    let mut settings = StatusLineSettings {
        path: preferences_path(options),
        ..StatusLineSettings::default()
    };
    let overridden = override_value.is_some();
    let loaded = override_value.map_or_else(
        || settings.read(),
        |value| {
            let value = value
                .into_string()
                .map_err(|_| format!("{STATUSLINE_ENV} must be Unicode"))?;
            StatusLine::parse(&value).map_err(|error| format!("{STATUSLINE_ENV} · {error}"))
        },
    );
    let (layout, problem) = match loaded {
        Ok(layout) => (layout, None),
        Err(error) => (StatusLine::default(), Some(error)),
    };
    if overridden {
        settings.override_line = Some(layout);
    } else {
        settings.saved = layout;
    }
    (settings, problem)
}

fn read_file(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        result => result,
    }
}

fn parse_file(content: &str) -> Result<StatusLine, String> {
    let config: TuiFile = toml::from_str(content).map_err(|error| error.message().to_owned())?;
    match config.statusline.and_then(|table| table.fields) {
        Some(fields) => StatusLine::from_fields(fields.iter().map(String::as_str)),
        None => Ok(StatusLine::default()),
    }
}

fn save_statusline(path: &Path, layout: &StatusLine) -> Result<(), String> {
    let content = read_file(path).map_err(|error| error.to_string())?;
    let mut file = content
        .parse::<DocumentMut>()
        .map_err(|error| error.message().to_owned())?;
    let statusline = file
        .as_table_mut()
        .entry("statusline")
        .or_insert_with(|| Item::Table(Table::new()));
    let table = statusline
        .as_table_like_mut()
        .ok_or("statusline must be a table")?;
    if let Some(fields) = layout.configured_names() {
        table.insert("fields", value(fields.into_iter().collect::<Array>()));
    } else {
        table.remove("fields");
    }
    let content = file.to_string();
    parse_file(&content)?;
    replace_file(path, &content).map_err(|error| error.to_string())
}

fn save_notifications(path: &Path, settings: Notifications) -> Result<(), String> {
    let content = read_file(path).map_err(|error| error.to_string())?;
    let mut file = content
        .parse::<DocumentMut>()
        .map_err(|error| error.message().to_owned())?;
    let table = file
        .as_table_mut()
        .entry("notifications")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or("notifications must be a table")?;
    for (name, enabled) in [
        ("question", settings.question),
        ("permission", settings.permission),
        ("interrupted", settings.interrupted),
        ("completed", settings.completed),
    ] {
        table.insert(name, value(enabled));
    }
    let content = file.to_string();
    parse_file(&content)?;
    replace_file(path, &content).map_err(|error| error.to_string())
}

fn replace_file(path: &Path, content: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;
    let mut file = NamedTempFile::new_in(directory)?;
    file.write_all(content.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map(|_| ()).map_err(|error| error.error)
}

fn file_problem(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{FILE_NAME} · {error} ({})", path.display())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use kuku_client::ConnectionOptions;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use tempfile::TempDir;

    use super::{FILE_NAME, STATUSLINE_ENV, resolve_statusline as load};
    use crate::app::{App, Modal};
    use crate::statusline::StatusLine;

    fn resolve_statusline(
        options: &ConnectionOptions,
        value: Option<OsString>,
    ) -> Result<StatusLine, String> {
        let (settings, problem) = load(options, value);
        problem.map_or_else(|| Ok(settings.active().clone()), Err)
    }

    #[test]
    fn notification_aliases_open_one_editor_and_save_local_preferences() {
        let data_dir = TempDir::new().unwrap();
        let file = data_dir.path().join(FILE_NAME);
        fs::write(&file, "# Keep this\n[statusline]\nfields = [\"agent\"]\n").unwrap();
        let options = ConnectionOptions {
            data_dir: Some(data_dir.path().to_owned()),
            remote: Some("https://service.example".into()),
            ..ConnectionOptions::default()
        };
        let (settings, error) = super::load_notifications(&options);
        assert!(error.is_none());
        assert!(settings.active().question);
        let mut app = App::new(PathBuf::from("/workspace"), options.remote.clone());
        app.notification_settings = settings;
        let buffer = Buffer::empty(Rect::new(0, 0, 100, 30));
        let press = |app: &mut App, code| {
            app.event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)), &buffer);
        };
        for command in ["/notifications", "/notify", "/sound"] {
            app.pane_mut().draft.set_text(command.into());
            press(&mut app, KeyCode::Enter);
            assert!(matches!(app.modal, Some(Modal::Notifications(_))));
            press(&mut app, KeyCode::Esc);
        }
        app.open_notifications();
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);
        let (saved, error) = super::load_notifications(&options);
        assert!(error.is_none());
        assert!(!saved.active().question);
        assert!(
            saved.active().permission && saved.active().interrupted && saved.active().completed
        );
        let contents = fs::read_to_string(file).unwrap();
        assert!(contents.contains("# Keep this") && contents.contains("fields = [\"agent\"]"));
    }

    #[test]
    fn local_and_remote_launches_read_statusline_from_client_data_dir() {
        let data_dir = TempDir::new().unwrap();
        let file = data_dir.path().join(FILE_NAME);
        fs::write(&file, "[statusline]\nfields = [\"workspace\", \"agent\"]").unwrap();
        let expected = StatusLine::parse("workspace,agent").unwrap();
        let mut options = ConnectionOptions {
            data_dir: Some(data_dir.path().to_owned()),
            workspace: Some(PathBuf::from("/workspace")),
            ..ConnectionOptions::default()
        };
        assert_eq!(expected, resolve_statusline(&options, None).unwrap());
        options.remote = Some("https://service.example".into());
        assert_eq!(expected, resolve_statusline(&options, None).unwrap());
    }

    #[test]
    fn empty_file_fields_disable_status_line_and_environment_takes_precedence() {
        let data_dir = TempDir::new().unwrap();
        let file = data_dir.path().join(FILE_NAME);
        let options = ConnectionOptions {
            data_dir: Some(data_dir.path().to_owned()),
            ..ConnectionOptions::default()
        };
        assert_eq!(
            StatusLine::default(),
            resolve_statusline(&options, None).unwrap()
        );
        fs::write(&file, "[statusline]\nfields = []").unwrap();
        assert_eq!(
            StatusLine::parse("none").unwrap(),
            resolve_statusline(&options, None).unwrap()
        );
        assert_eq!(
            StatusLine::parse("agent,workspace,context").unwrap(),
            resolve_statusline(&options, Some("agent,workspace,context".into())).unwrap(),
        );
        assert_eq!(
            StatusLine::default(),
            resolve_statusline(&options, Some(" ".into())).unwrap()
        );
    }

    #[test]
    fn invalid_configuration_reports_the_selected_source_and_reason() {
        let data_dir = TempDir::new().unwrap();
        let file = data_dir.path().join(FILE_NAME);
        let options = ConnectionOptions {
            data_dir: Some(data_dir.path().to_owned()),
            ..ConnectionOptions::default()
        };
        fs::write(&file, "[statusline]\nfields = [\"stats\"]").unwrap();
        let problem = resolve_statusline(&options, None).unwrap_err();
        assert!(problem.contains("tui.toml") && problem.contains("unknown field \"stats\""));
        let problem = resolve_statusline(&options, Some("agent,agent".into())).unwrap_err();
        assert!(problem.contains(STATUSLINE_ENV) && problem.contains("repeats agent"));
        fs::write(&file, "[statusline]\nfields = \"agent\"").unwrap();
        let problem = resolve_statusline(&options, None).unwrap_err();
        assert!(problem.contains("tui.toml") && problem.contains("sequence"));
        assert_eq!(
            StatusLine::parse("none").unwrap(),
            resolve_statusline(&options, Some("none".into())).unwrap(),
        );
    }

    #[test]
    fn statusline_editor_previews_cancels_saves_and_restores_the_file_layout() {
        let directory = TempDir::new().unwrap();
        let file = directory.path().join(FILE_NAME);
        let initial =
            "# personal layout\n[statusline]\nfields = [\"agent\", \"workspace\", \"context\"]\n";
        fs::write(&file, initial).unwrap();
        let options = ConnectionOptions {
            data_dir: Some(directory.path().to_owned()),
            ..ConnectionOptions::default()
        };
        let (settings, warning) = load(&options, None);
        assert!(warning.is_none());
        let mut app = App::new(PathBuf::from("/workspace"), None);
        app.statusline_settings = settings;

        open_editor(&mut app);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(
            Some(vec!["workspace", "context"]),
            app.displayed_statusline().configured_names()
        );
        assert_eq!(initial, fs::read_to_string(&file).unwrap());
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            Some(vec!["agent", "workspace", "context"]),
            app.displayed_statusline().configured_names()
        );

        open_editor(&mut app);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.modal.is_none());
        let reordered = StatusLine::parse("workspace,agent,context").unwrap();
        assert_eq!(&reordered, app.displayed_statusline());
        assert_eq!(reordered, resolve_statusline(&options, None).unwrap());
        let saved = fs::read_to_string(&file).unwrap();
        assert!(saved.starts_with("# personal layout"));

        open_editor(&mut app);
        press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE);
        assert_eq!(&StatusLine::default(), app.displayed_statusline());
        assert_eq!(saved, fs::read_to_string(&file).unwrap());
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            StatusLine::default(),
            resolve_statusline(&options, None).unwrap()
        );
        let restored = fs::read_to_string(&file).unwrap();
        assert!(restored.starts_with("# personal layout") && !restored.contains("fields"));
    }

    #[test]
    fn saved_preferences_under_an_environment_override_apply_without_the_override() {
        let directory = TempDir::new().unwrap();
        let file = directory.path().join(FILE_NAME);
        fs::write(&file, "[statusline]\nfields = [\"workspace\"]\n").unwrap();
        let options = ConnectionOptions {
            data_dir: Some(directory.path().to_owned()),
            remote: Some("https://service.example".into()),
            ..ConnectionOptions::default()
        };
        let (settings, warning) = load(&options, Some("agent".into()));
        assert!(warning.is_none());
        let mut app = App::new(PathBuf::from("/workspace"), options.remote.clone());
        app.statusline_settings = settings;

        open_editor(&mut app);
        assert_eq!(
            Some(vec!["workspace"]),
            app.displayed_statusline().configured_names()
        );
        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            Some(vec!["agent"]),
            app.displayed_statusline().configured_names()
        );
        assert_eq!(
            Some(Vec::new()),
            resolve_statusline(&options, None)
                .unwrap()
                .configured_names()
        );
    }

    #[test]
    fn failed_save_keeps_the_editor_and_preserves_the_invalid_file() {
        let directory = TempDir::new().unwrap();
        let file = directory.path().join(FILE_NAME);
        let contents = "[statusline\nfields = [";
        fs::write(&file, contents).unwrap();
        let options = ConnectionOptions {
            data_dir: Some(directory.path().to_owned()),
            ..ConnectionOptions::default()
        };
        let (settings, warning) = load(&options, None);
        assert!(warning.is_some());
        let mut app = App::new(PathBuf::from("/workspace"), None);
        app.statusline_settings = settings;
        open_editor(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.modal, Some(Modal::StatusLineEditor(_))));
        assert_eq!(contents, fs::read_to_string(file).unwrap());
    }

    fn open_editor(app: &mut App) {
        press(app, KeyCode::Char('/'), KeyModifiers::NONE);
        app.event(
            Event::Paste("/statusline".into()),
            &Buffer::empty(Rect::ZERO),
        );
        press(app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.modal, Some(Modal::StatusLineEditor(_))));
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.event(
            Event::Key(KeyEvent::new(code, modifiers)),
            &Buffer::empty(Rect::ZERO),
        );
    }
}
