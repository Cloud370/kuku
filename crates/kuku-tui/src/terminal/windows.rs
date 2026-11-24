use std::io;
use std::mem::MaybeUninit;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::thread::{self, JoinHandle};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use termina::Parser;
use termina::event::{
    KeyCode as VirtualKey, KeyEventKind as VirtualKeyKind, Modifiers, MouseButton as VirtualButton,
    MouseEventKind as VirtualMouseKind,
};
use tokio::sync::mpsc;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, COORD, DOUBLE_CLICK, ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS,
    ENABLE_LINE_INPUT, ENABLE_MOUSE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT,
    ENABLE_QUICK_EDIT_MODE, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    ENABLE_WINDOW_INPUT, FOCUS_EVENT, FROM_LEFT_1ST_BUTTON_PRESSED, GetConsoleMode,
    GetConsoleScreenBufferInfo, INPUT_RECORD, KEY_EVENT, LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED,
    MOUSE_EVENT, MOUSE_EVENT_RECORD, MOUSE_MOVED, MOUSE_WHEELED, RIGHT_ALT_PRESSED,
    RIGHT_CTRL_PRESSED, ReadConsoleInputW, SHIFT_PRESSED, SetConsoleMode, WINDOW_BUFFER_SIZE_EVENT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_MENU;

const INPUT_RECORDS: usize = 128;
const EVENT_QUEUE: usize = 64;
const ESCAPE_TIMEOUT_MS: u32 = 35;

pub(super) struct ConsoleModes {
    input: u32,
    output: u32,
}

impl ConsoleModes {
    pub(super) fn enter() -> io::Result<Self> {
        let input = io::stdin().as_raw_handle();
        let output = io::stdout().as_raw_handle();
        let saved = Self {
            input: mode(input)?,
            output: mode(output)?,
        };
        let cooked =
            ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT | ENABLE_QUICK_EDIT_MODE;
        let raw = ENABLE_VIRTUAL_TERMINAL_INPUT
            | ENABLE_WINDOW_INPUT
            | ENABLE_MOUSE_INPUT
            | ENABLE_EXTENDED_FLAGS;
        set_mode(input, (saved.input & !cooked) | raw)?;
        set_mode(
            output,
            saved.output | ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT,
        )?;
        Ok(saved)
    }
}

impl Drop for ConsoleModes {
    fn drop(&mut self) {
        let _ = set_mode(io::stdin().as_raw_handle(), self.input);
        let _ = set_mode(io::stdout().as_raw_handle(), self.output);
    }
}

pub(super) struct Input {
    receiver: mpsc::Receiver<io::Result<Event>>,
    stop: OwnedHandle,
    worker: Option<JoinHandle<()>>,
}

impl Input {
    pub(super) fn new() -> io::Result<Self> {
        let stop = stop_event()?;
        let wake = stop.try_clone()?;
        let (sender, receiver) = mpsc::channel(EVENT_QUEUE);
        let worker = thread::Builder::new()
            .name("kuku-console-input".into())
            .spawn(move || {
                if let Err(error) = read_events(&wake, &sender) {
                    let _ = sender.blocking_send(Err(error));
                }
            })?;
        Ok(Self {
            receiver,
            stop,
            worker: Some(worker),
        })
    }

    pub(super) async fn next(&mut self) -> Option<io::Result<Event>> {
        self.receiver.recv().await
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.receiver.close();
        // SAFETY: The owned event remains open until the worker leaves its wait.
        unsafe { SetEvent(self.stop.as_raw_handle()) };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_events(stop: &OwnedHandle, sender: &mpsc::Sender<io::Result<Event>>) -> io::Result<()> {
    let input = io::stdin().as_raw_handle();
    let handles = [stop.as_raw_handle(), input];
    let mut decoder = Decoder {
        viewport_origin: viewport_origin(),
        ..Decoder::default()
    };
    let mut records = [MaybeUninit::<INPUT_RECORD>::uninit(); INPUT_RECORDS];
    let mut timeout = INFINITE;
    loop {
        // SAFETY: Both handles remain valid for this entire wait. The stop event takes priority over unread input.
        let ready =
            unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout) };
        if ready == WAIT_OBJECT_0 {
            return Ok(());
        }
        if ready == WAIT_TIMEOUT {
            decoder.finish_batch();
            timeout = INFINITE;
        } else if ready == WAIT_OBJECT_0 + 1 {
            let mut count = 0;
            // SAFETY: The array has space for the requested records, and only the initialized prefix is read.
            if unsafe {
                ReadConsoleInputW(
                    input,
                    records.as_mut_ptr().cast(),
                    INPUT_RECORDS as u32,
                    &mut count,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            for record in &records[..count as usize] {
                // SAFETY: A successful read initialized exactly `count` records.
                if let Some(event) = decoder.record(unsafe { record.assume_init_ref() }) {
                    if sender.blocking_send(Ok(event)).is_err() {
                        return Ok(());
                    }
                }
                if !decoder.deliver(sender) {
                    return Ok(());
                }
            }
            timeout = if decoder.trailing_escape {
                ESCAPE_TIMEOUT_MS
            } else {
                INFINITE
            };
        } else if ready == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        } else {
            return Err(io::Error::other("unexpected Windows console wait result"));
        }
        if !decoder.deliver(sender) {
            return Ok(());
        }
    }
}

#[derive(Default)]
struct Decoder {
    parser: Parser,
    high_surrogate: Option<u16>,
    trailing_escape: bool,
    left_down: bool,
    viewport_origin: COORD,
}

impl Decoder {
    fn record(&mut self, record: &INPUT_RECORD) -> Option<Event> {
        match u32::from(record.EventType) {
            KEY_EVENT => {
                // SAFETY: The event type identifies the union member, and ReadConsoleInputW returns UTF-16.
                let key = unsafe { record.Event.KeyEvent };
                // SAFETY: ReadConsoleInputW populated the Unicode member of the key record.
                let unit = unsafe { key.uChar.UnicodeChar };
                if unit != 0 && (key.bKeyDown != 0 || key.wVirtualKeyCode == VK_MENU) {
                    for _ in 0..key.wRepeatCount.max(1) {
                        self.push(unit);
                    }
                }
                None
            }
            WINDOW_BUFFER_SIZE_EVENT => {
                // SAFETY: The event type identifies the window-size union member.
                let size = unsafe { record.Event.WindowBufferSizeEvent }.dwSize;
                self.viewport_origin = viewport_origin();
                Some(Event::Resize(size.X as u16, size.Y as u16))
            }
            FOCUS_EVENT => {
                // SAFETY: The event type identifies the focus union member.
                let focused = unsafe { record.Event.FocusEvent }.bSetFocus != 0;
                Some(if focused {
                    Event::FocusGained
                } else {
                    self.left_down = false;
                    Event::FocusLost
                })
            }
            MOUSE_EVENT => {
                // SAFETY: The event type identifies the mouse union member.
                let mouse = unsafe { record.Event.MouseEvent };
                self.native_mouse(mouse)
            }
            _ => None,
        }
    }

    fn native_mouse(&mut self, mouse: MOUSE_EVENT_RECORD) -> Option<Event> {
        let down = mouse.dwButtonState & FROM_LEFT_1ST_BUTTON_PRESSED != 0;
        let was_down = std::mem::replace(&mut self.left_down, down);
        let kind = match (mouse.dwEventFlags, down, was_down) {
            (0 | DOUBLE_CLICK, true, false) => MouseEventKind::Down(MouseButton::Left),
            (0, false, true) => MouseEventKind::Up(MouseButton::Left),
            (MOUSE_MOVED, true, _) => MouseEventKind::Drag(MouseButton::Left),
            (MOUSE_MOVED, false, true) => MouseEventKind::Moved,
            (MOUSE_WHEELED, _, _) => {
                let delta = (mouse.dwButtonState >> 16) as i16;
                if delta < 0 {
                    MouseEventKind::ScrollDown
                } else {
                    MouseEventKind::ScrollUp
                }
            }
            _ => return None,
        };
        let state = mouse.dwControlKeyState;
        let mut modifiers = KeyModifiers::NONE;
        modifiers.set(KeyModifiers::SHIFT, state & SHIFT_PRESSED != 0);
        modifiers.set(
            KeyModifiers::ALT,
            state & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0,
        );
        modifiers.set(
            KeyModifiers::CONTROL,
            state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0,
        );
        let origin = self.viewport_origin;
        let position = mouse.dwMousePosition;
        Some(mouse_event(
            kind,
            position.X.saturating_sub(origin.X).max(0) as u16,
            position.Y.saturating_sub(origin.Y).max(0) as u16,
            modifiers,
        ))
    }

    fn push(&mut self, unit: u16) {
        let high = self.high_surrogate.take();
        match unit {
            0xd800..=0xdbff => {
                self.high_surrogate = Some(unit);
                if high.is_some() {
                    self.character(char::REPLACEMENT_CHARACTER);
                }
            }
            0xdc00..=0xdfff => {
                let character = high.and_then(|high| char::decode_utf16([high, unit]).next()?.ok());
                self.character(character.unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            _ => {
                if high.is_some() {
                    self.character(char::REPLACEMENT_CHARACTER);
                }
                self.character(char::from_u32(u32::from(unit)).expect("non-surrogate input"));
            }
        }
    }

    fn character(&mut self, character: char) {
        let escape = character == '\x1b';
        if escape && self.trailing_escape {
            self.finish_batch();
        }
        self.trailing_escape = escape;
        self.parser
            .parse(character.encode_utf8(&mut [0; 4]).as_bytes(), true);
    }

    fn finish_batch(&mut self) {
        // NOTE: Completing a standalone Escape leaves incomplete CSI and paste data buffered.
        if std::mem::take(&mut self.trailing_escape) {
            self.parser.parse(&[], false);
        }
    }

    fn next(&mut self) -> Option<Event> {
        std::iter::from_fn(|| self.parser.pop()).find_map(translate)
    }

    fn deliver(&mut self, sender: &mpsc::Sender<io::Result<Event>>) -> bool {
        while let Some(event) = self.next() {
            if sender.blocking_send(Ok(event)).is_err() {
                return false;
            }
        }
        true
    }
}

fn translate(event: termina::Event) -> Option<Event> {
    match event {
        termina::Event::Paste(text) => Some(Event::Paste(text)),
        termina::Event::WindowResized(size) => Some(Event::Resize(size.cols, size.rows)),
        termina::Event::FocusIn => Some(Event::FocusGained),
        termina::Event::FocusOut => Some(Event::FocusLost),
        termina::Event::Key(event) => {
            let code = key_code(event.code)?;
            let kind = match event.kind {
                VirtualKeyKind::Press => KeyEventKind::Press,
                VirtualKeyKind::Release => KeyEventKind::Release,
                VirtualKeyKind::Repeat => KeyEventKind::Repeat,
            };
            Some(Event::Key(KeyEvent::new_with_kind(
                code,
                modifiers(event.modifiers),
                kind,
            )))
        }
        termina::Event::Mouse(event) => {
            let kind = match event.kind {
                VirtualMouseKind::Down(VirtualButton::Left) => {
                    MouseEventKind::Down(MouseButton::Left)
                }
                VirtualMouseKind::Drag(VirtualButton::Left) => {
                    MouseEventKind::Drag(MouseButton::Left)
                }
                VirtualMouseKind::Up(VirtualButton::Left) => MouseEventKind::Up(MouseButton::Left),
                VirtualMouseKind::Moved => MouseEventKind::Moved,
                VirtualMouseKind::ScrollUp => MouseEventKind::ScrollUp,
                VirtualMouseKind::ScrollDown => MouseEventKind::ScrollDown,
                VirtualMouseKind::Down(VirtualButton::Middle | VirtualButton::Right)
                | VirtualMouseKind::Drag(VirtualButton::Middle | VirtualButton::Right)
                | VirtualMouseKind::Up(VirtualButton::Middle | VirtualButton::Right)
                | VirtualMouseKind::ScrollLeft
                | VirtualMouseKind::ScrollRight => return None,
            };
            Some(mouse_event(
                kind,
                event.column,
                event.row,
                modifiers(event.modifiers),
            ))
        }
        termina::Event::Csi(_) | termina::Event::Osc(_) | termina::Event::Dcs(_) => None,
    }
}

fn key_code(code: VirtualKey) -> Option<KeyCode> {
    Some(match code {
        VirtualKey::Char(character) => KeyCode::Char(character),
        VirtualKey::Enter => KeyCode::Enter,
        VirtualKey::Backspace => KeyCode::Backspace,
        VirtualKey::Tab => KeyCode::Tab,
        VirtualKey::Escape => KeyCode::Esc,
        VirtualKey::Left => KeyCode::Left,
        VirtualKey::Right => KeyCode::Right,
        VirtualKey::Up => KeyCode::Up,
        VirtualKey::Down => KeyCode::Down,
        VirtualKey::Home => KeyCode::Home,
        VirtualKey::End => KeyCode::End,
        VirtualKey::BackTab => KeyCode::BackTab,
        VirtualKey::PageUp => KeyCode::PageUp,
        VirtualKey::PageDown => KeyCode::PageDown,
        VirtualKey::Insert => KeyCode::Insert,
        VirtualKey::Delete => KeyCode::Delete,
        VirtualKey::Function(number) => KeyCode::F(number),
        VirtualKey::KeypadBegin
        | VirtualKey::CapsLock
        | VirtualKey::ScrollLock
        | VirtualKey::NumLock
        | VirtualKey::PrintScreen
        | VirtualKey::Pause
        | VirtualKey::Menu
        | VirtualKey::Null
        | VirtualKey::Modifier(_)
        | VirtualKey::Media(_) => return None,
    })
}

fn modifiers(native: Modifiers) -> KeyModifiers {
    let mut converted = KeyModifiers::NONE;
    converted.set(KeyModifiers::SHIFT, native.contains(Modifiers::SHIFT));
    converted.set(KeyModifiers::CONTROL, native.contains(Modifiers::CONTROL));
    converted.set(KeyModifiers::ALT, native.contains(Modifiers::ALT));
    converted.set(KeyModifiers::SUPER, native.contains(Modifiers::SUPER));
    converted.set(KeyModifiers::HYPER, native.contains(Modifiers::HYPER));
    converted.set(KeyModifiers::META, native.contains(Modifiers::META));
    converted
}

fn mouse_event(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    })
}

fn viewport_origin() -> COORD {
    let mut screen = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: The process owns the output handle, and `screen` is writable during the call.
    if unsafe { GetConsoleScreenBufferInfo(io::stdout().as_raw_handle(), &mut screen) } == 0 {
        return COORD::default();
    }
    COORD {
        X: screen.srWindow.Left,
        Y: screen.srWindow.Top,
    }
}

fn stop_event() -> io::Result<OwnedHandle> {
    // SAFETY: The event has default security, starts unsignaled, and its returned handle is owned here.
    let handle = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateEventW returned a valid, individually owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn mode(handle: HANDLE) -> io::Result<u32> {
    let mut mode = 0;
    // SAFETY: Standard console handles are borrowed from the process. `mode` is writable for this call.
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(mode)
}

fn set_mode(handle: HANDLE, mode: u32) -> io::Result<()> {
    // SAFETY: The console handle is a process standard handle, and GetConsoleMode verified it.
    if unsafe { SetConsoleMode(handle, mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::System::Console::{
        INPUT_RECORD_0, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0, RIGHTMOST_BUTTON_PRESSED,
    };

    use super::{
        COORD, Decoder, Event, FROM_LEFT_1ST_BUTTON_PRESSED, INPUT_RECORD, KEY_EVENT, KeyCode,
        KeyEvent, KeyModifiers, MOUSE_EVENT, MOUSE_EVENT_RECORD, MOUSE_MOVED, MOUSE_WHEELED,
        MouseButton, MouseEventKind, SHIFT_PRESSED, VK_MENU, mouse_event,
    };

    fn input(decoder: &mut Decoder, text: &str) {
        for unit in text.encode_utf16() {
            decoder.record(&event(unit, true, 0));
        }
    }

    fn event(unit: u16, down: bool, virtual_key: u16) -> INPUT_RECORD {
        INPUT_RECORD {
            EventType: KEY_EVENT as u16,
            Event: INPUT_RECORD_0 {
                KeyEvent: KEY_EVENT_RECORD {
                    bKeyDown: i32::from(down),
                    wRepeatCount: 1,
                    wVirtualKeyCode: virtual_key,
                    wVirtualScanCode: 0,
                    uChar: KEY_EVENT_RECORD_0 { UnicodeChar: unit },
                    dwControlKeyState: 0,
                },
            },
        }
    }

    #[test]
    fn console_paste_retains_mixed_line_endings_and_unicode_until_end_marker() {
        let mut decoder = Decoder::default();
        input(&mut decoder, "\x1b[20");
        decoder.finish_batch();
        input(&mut decoder, "0~中文\r\ne\u{301}\r");
        for unit in "👩‍💻".encode_utf16() {
            decoder.record(&event(unit, false, VK_MENU));
        }
        input(&mut decoder, "\nend\x1b[20");
        decoder.finish_batch();
        assert_eq!(None, decoder.next());
        input(&mut decoder, "1~");
        assert_eq!(
            Some(Event::Paste("中文\r\ne\u{301}\r👩‍💻\nend".into())),
            decoder.next()
        );
        assert_eq!(None, decoder.next());
        input(&mut decoder, "\r");
        assert_eq!(Some(Event::Key(KeyCode::Enter.into())), decoder.next());
    }

    #[test]
    fn standalone_escape_in_the_same_batch_keeps_the_next_paste_marker_intact() {
        let mut decoder = Decoder::default();
        input(
            &mut decoder,
            "\x1b\x1b[200~first\r\nsecond\x1b\x1bline\x1b[201~",
        );
        assert_eq!(Some(Event::Key(KeyCode::Esc.into())), decoder.next());
        assert_eq!(
            Some(Event::Paste("first\r\nsecond\x1b\x1bline".into())),
            decoder.next()
        );
        assert_eq!(None, decoder.next());
    }

    #[test]
    fn virtual_input_preserves_navigation_and_enter_modifiers() {
        let mut decoder = Decoder::default();
        input(&mut decoder, "\x1b");
        assert_eq!(None, decoder.next());
        decoder.finish_batch();
        assert_eq!(Some(Event::Key(KeyCode::Esc.into())), decoder.next());
        input(&mut decoder, "\x1b[D\x1b[13;2u\n\x1b\r");
        let output: Vec<_> = std::iter::from_fn(|| decoder.next()).collect();
        assert_eq!(
            vec![
                Event::Key(KeyCode::Left.into()),
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
                Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)),
            ],
            output
        );
    }

    #[test]
    fn native_mouse_reports_convert_button_transitions_and_viewport_offsets() {
        let mut decoder = Decoder {
            viewport_origin: COORD { X: 2, Y: 3 },
            ..Decoder::default()
        };
        let left = MouseButton::Left;
        let pressed = FROM_LEFT_1ST_BUTTON_PRESSED;
        let wheel = u32::from((-120i16) as u16) << 16;
        for (flags, buttons, state, expected) in [
            (0, pressed, 0, Some(MouseEventKind::Down(left))),
            (0, pressed | RIGHTMOST_BUTTON_PRESSED, 0, None),
            (
                MOUSE_MOVED,
                pressed,
                SHIFT_PRESSED,
                Some(MouseEventKind::Drag(left)),
            ),
            (
                MOUSE_WHEELED,
                pressed | wheel,
                0,
                Some(MouseEventKind::ScrollDown),
            ),
            (0, 0, 0, Some(MouseEventKind::Up(left))),
            (MOUSE_MOVED, 0, 0, None),
        ] {
            let record = INPUT_RECORD {
                EventType: MOUSE_EVENT as u16,
                Event: INPUT_RECORD_0 {
                    MouseEvent: MOUSE_EVENT_RECORD {
                        dwMousePosition: COORD { X: 7, Y: 9 },
                        dwButtonState: buttons,
                        dwControlKeyState: state,
                        dwEventFlags: flags,
                    },
                },
            };
            let modifiers = if state == SHIFT_PRESSED {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            assert_eq!(
                expected.map(|kind| mouse_event(kind, 5, 6, modifiers)),
                decoder.record(&record)
            );
        }
    }

    #[test]
    fn virtual_mouse_reports_preserve_left_drag_wheel_and_modifiers() {
        let mut decoder = Decoder::default();
        input(
            &mut decoder,
            "\x1b[<0;3;4M\x1b[<32;4;5M\x1b[<0;5;6m\x1b[<65;5;6M\x1b[<4;3;4M",
        );
        let output: Vec<_> = std::iter::from_fn(|| decoder.next()).collect();
        let left = MouseButton::Left;
        let none = KeyModifiers::NONE;
        assert_eq!(
            vec![
                mouse_event(MouseEventKind::Down(left), 2, 3, none),
                mouse_event(MouseEventKind::Drag(left), 3, 4, none),
                mouse_event(MouseEventKind::Up(left), 4, 5, none),
                mouse_event(MouseEventKind::ScrollDown, 4, 5, none),
                mouse_event(MouseEventKind::Down(left), 2, 3, KeyModifiers::SHIFT),
            ],
            output
        );
    }
}
