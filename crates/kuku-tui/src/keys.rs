use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy)]
pub(crate) enum AgentDirection {
    Previous,
    Next,
}

impl AgentDirection {
    pub(crate) fn from_key(key: KeyEvent) -> Option<Self> {
        match (key.modifiers, key.code) {
            (KeyModifiers::ALT, KeyCode::Left) => Some(Self::Previous),
            (KeyModifiers::ALT, KeyCode::Right) => Some(Self::Next),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Shortcut {
    code: KeyCode,
    modifiers: KeyModifiers,
    pub(crate) label: &'static str,
}

impl Shortcut {
    pub(crate) const ACTIONS: Self = Self::control('p', "Ctrl+P");
    pub(crate) const ACTIONS_ALIAS: Self = Self::plain(KeyCode::F(1), "F1");
    pub(crate) const REPLY: Self = Self::plain(KeyCode::F(2), "F2");
    pub(crate) const PERMISSIONS: Self = Self::plain(KeyCode::F(3), "F3");
    pub(crate) const ESCAPE: Self = Self::plain(KeyCode::Esc, "Esc");
    pub(crate) const ENTER: Self = Self::plain(KeyCode::Enter, "Enter");
    pub(crate) const TAB: Self = Self::plain(KeyCode::Tab, "Tab");
    pub(crate) const UP: Self = Self::plain(KeyCode::Up, "↑");
    pub(crate) const DOWN: Self = Self::plain(KeyCode::Down, "↓");
    pub(crate) const PREVIOUS_PROMPT: Self =
        Self::new(KeyCode::Up, KeyModifiers::CONTROL, "Ctrl+↑");
    pub(crate) const NEXT_PROMPT: Self = Self::new(KeyCode::Down, KeyModifiers::CONTROL, "Ctrl+↓");
    pub(crate) const PAGE_UP: Self = Self::plain(KeyCode::PageUp, "PgUp");
    pub(crate) const PAGE_DOWN: Self = Self::plain(KeyCode::PageDown, "PgDn");
    pub(crate) const REFRESH_OUTPUT: Self = Self::plain(KeyCode::Char('r'), "R");
    pub(crate) const NAME_OUTPUT: Self = Self::plain(KeyCode::Char('s'), "S");
    pub(crate) const NEW_LINE: Self = Self::new(KeyCode::Enter, KeyModifiers::SHIFT, "Shift+Enter");
    pub(crate) const CONTROL_NEW_LINE: Self = Self::control('j', "Ctrl+J");
    pub(crate) const NEXT_TURN: Self = Self::new(KeyCode::Enter, KeyModifiers::ALT, "Alt+Enter");
    pub(crate) const PASTE: Self = Self::control('v', "Ctrl+V");
    pub(crate) const PASTE_IMAGE_ALIAS: Self = Self::new(
        KeyCode::Char('v'),
        KeyModifiers::CONTROL.union(KeyModifiers::ALT),
        "Ctrl+Alt+V",
    );
    pub(crate) const CLEAR: Self = Self::control('c', "Ctrl+C");
    pub(crate) const RECONNECT: Self = Self::control('r', "Ctrl+R");
    pub(crate) const REWIND: Self = Self::control('g', "Ctrl+G");
    pub(crate) const MODEL: Self = Self::control('l', "Ctrl+L");
    pub(crate) const CYCLE_THINKING: Self = Self::control('t', "Ctrl+T");
    pub(crate) const DETAILS: Self = Self::control('o', "Ctrl+O");
    pub(crate) const LATEST: Self = Self::new(KeyCode::End, KeyModifiers::CONTROL, "Ctrl+End");
    pub(crate) const READING_BACK: Self = Self::new(KeyCode::Up, KeyModifiers::ALT, "Alt+↑");

    pub(crate) fn matches(self, key: KeyEvent) -> bool {
        key.code == self.code && key.modifiers == self.modifiers
    }

    const fn plain(code: KeyCode, label: &'static str) -> Self {
        Self::new(code, KeyModifiers::NONE, label)
    }

    const fn control(character: char, label: &'static str) -> Self {
        Self::new(KeyCode::Char(character), KeyModifiers::CONTROL, label)
    }

    const fn new(code: KeyCode, modifiers: KeyModifiers, label: &'static str) -> Self {
        Self {
            code,
            modifiers,
            label,
        }
    }
}
