use crate::keys::Shortcut;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CommandMenu {
    New,
    Resume,
    Agents,
    Jobs,
    Model,
    Thinking,
    StatusLine,
    Save,
    Values,
    Server,
    Restart,
    Web,
    Quit,
}

impl CommandMenu {
    pub(crate) const ALL: &[Self] = &[
        Self::New,
        Self::Resume,
        Self::Agents,
        Self::Jobs,
        Self::Model,
        Self::Thinking,
        Self::StatusLine,
        Self::Save,
        Self::Values,
        Self::Server,
        Self::Restart,
        Self::Web,
        Self::Quit,
    ];

    pub(crate) fn name(self) -> &'static str {
        self.spec().0
    }

    pub(crate) fn description(self) -> String {
        let (_, _, description, shortcut) = self.spec();
        shortcut.map_or_else(
            || description.to_owned(),
            |shortcut| format!("{description} · {}", shortcut.label),
        )
    }

    pub(crate) fn starts_with(self, prefix: &str) -> bool {
        self.names().any(|name| name.starts_with(prefix))
    }

    fn names(self) -> impl Iterator<Item = &'static str> {
        let (name, aliases, _, _) = self.spec();
        std::iter::once(name).chain(aliases.iter().copied())
    }

    fn spec(
        self,
    ) -> (
        &'static str,
        &'static [&'static str],
        &'static str,
        Option<Shortcut>,
    ) {
        match self {
            Self::New => ("new", &["new-session"], "Open a fresh draft", None),
            Self::Resume => (
                "resume",
                &["sessions"],
                "Find a Session in this workspace",
                None,
            ),
            Self::Agents => ("agents", &["agent"], "Choose or create an Agent", None),
            Self::Jobs => ("jobs", &["job"], "List jobs · /jobs cancel <id>", None),
            Self::Model => (
                "model",
                &["models"],
                "Choose a model route",
                Some(Shortcut::MODEL),
            ),
            Self::Thinking => (
                "thinking",
                &["think", "effort"],
                "Select thinking effort",
                Some(Shortcut::CYCLE_THINKING),
            ),
            Self::StatusLine => ("statusline", &["status-line"], "Edit the status line", None),
            Self::Save => (
                "save",
                &["save-value"],
                "Name pasted text or the draft",
                None,
            ),
            Self::Values => (
                "values",
                &["value", "saved-values"],
                "Browse saved values",
                None,
            ),
            Self::Server => (
                "server",
                &["service"],
                "Inspect the connected service",
                None,
            ),
            Self::Restart => (
                "restart",
                &["restart-service"],
                "Confirm a local service restart",
                None,
            ),
            Self::Web => ("web", &["browser"], "Open in the browser", None),
            Self::Quit => ("quit", &["exit", "q"], "Leave the terminal", None),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CommandInput<'a> {
    pub(crate) name: &'a str,
    pub(crate) arguments: &'a str,
    text: &'a str,
}

impl<'a> CommandInput<'a> {
    pub(crate) fn draft(text: &'a str) -> Option<Self> {
        if text.contains(['\r', '\n']) {
            return None;
        }
        let text = text.trim_start();
        text.starts_with('/').then(|| Self::query(text))
    }

    pub(crate) fn query(text: &'a str) -> Self {
        let text = text.trim_start();
        let text = text.strip_prefix('/').unwrap_or(text);
        let end = text.find(char::is_whitespace).unwrap_or(text.len());
        Self {
            name: &text[..end],
            arguments: &text[end..],
            text,
        }
    }

    pub(crate) fn text(self) -> &'a str {
        self.text
    }

    pub(crate) fn command(self) -> Option<CommandMenu> {
        CommandMenu::ALL
            .iter()
            .copied()
            .find(|command| command.names().any(|name| name == self.name))
    }

    pub(crate) fn complete(self, command: CommandMenu) -> String {
        format!(
            "/{}{args}",
            command.name(),
            args = self.arguments_for(command)
        )
    }

    pub(crate) fn arguments_for(self, command: CommandMenu) -> &'a str {
        if command.starts_with(&self.name.to_ascii_lowercase()) {
            self.arguments
        } else {
            ""
        }
    }
}
