use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::Path;

use kuku_client::ConnectionOptions;
use serde::Deserialize;

use crate::statusline::StatusLine;

const FILE_NAME: &str = "tui.toml";
const STATUSLINE_ENV: &str = "KUKU_TUI_STATUSLINE";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TuiFile {
    statusline: Option<StatusLineFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusLineFile {
    fields: Option<Vec<String>>,
}

pub(crate) fn load_statusline(options: &ConnectionOptions) -> Result<StatusLine, String> {
    resolve_statusline(options, env::var_os(STATUSLINE_ENV))
}

fn resolve_statusline(
    options: &ConnectionOptions,
    override_value: Option<OsString>,
) -> Result<StatusLine, String> {
    if let Some(value) = override_value {
        let value = value
            .into_string()
            .map_err(|_| format!("{STATUSLINE_ENV} must be Unicode"))?;
        return StatusLine::parse(&value).map_err(|error| format!("{STATUSLINE_ENV} · {error}"));
    }

    let path = options
        .data_dir()
        .map_err(|error| format!("Locate {FILE_NAME} · {error}"))?
        .join(FILE_NAME);
    read_statusline(&path).map_err(|error| format!("{FILE_NAME} · {error} ({})", path.display()))
}

fn read_statusline(path: &Path) -> Result<StatusLine, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(StatusLine::default()),
        Err(error) => return Err(error.to_string()),
    };
    let config: TuiFile = toml::from_str(&content).map_err(|error| error.message().to_owned())?;
    match config.statusline.and_then(|table| table.fields) {
        Some(fields) => StatusLine::from_fields(fields.iter().map(String::as_str)),
        None => Ok(StatusLine::default()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use kuku_client::ConnectionOptions;
    use tempfile::TempDir;

    use super::{resolve_statusline, FILE_NAME, STATUSLINE_ENV};
    use crate::statusline::StatusLine;

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
}
