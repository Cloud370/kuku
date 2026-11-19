//! Full-screen terminal client for kuku's Session service.

use std::io::{self, IsTerminal};

use kuku_client::ConnectionOptions;

mod app;
mod browser;
mod config;
mod contextual;
mod editor;
mod interaction;
mod jobs;
mod keys;
mod markup;
mod menu;
mod output;
mod selection;
mod statusline;
mod terminal;
mod text;
mod theme;
mod transcript;
mod ui;

/// Run the interactive Session client using the supplied connection and workspace selection.
pub async fn run(options: ConnectionOptions) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Interactive kuku needs a terminal. Use `kuku run` for scripts.",
        ));
    }
    terminal::run(options).await
}

/// Open an HTTP or HTTPS URL in the user's browser.
pub fn open_browser(url: &str) -> io::Result<()> {
    browser::open_with(url, |url| open::that_detached(url))
}
