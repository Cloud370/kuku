use std::io;
use std::mem::MaybeUninit;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::thread::{self, JoinHandle};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use termina::event::{KeyCode as VirtualKey, KeyEventKind as VirtualKeyKind, Modifiers};
use termina::Parser;
use tokio::sync::mpsc;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Console::{
    GetConsoleMode, ReadConsoleInputW, SetConsoleMode, ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS,
    ENABLE_LINE_INPUT, ENABLE_MOUSE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT,
    ENABLE_QUICK_EDIT_MODE, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    ENABLE_WINDOW_INPUT, FOCUS_EVENT, INPUT_RECORD, KEY_EVENT, MOUSE_EVENT, MOUSE_WHEELED,
    WINDOW_BUFFER_SIZE_EVENT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE,
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
        // NOTE: The owned event remains open until the worker leaves its wait.
        unsafe { SetEvent(self.stop.as_raw_handle()) };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_events(stop: &OwnedHandle, sender: &mpsc::Sender<io::Result<Event>>) -> io::Result<()> {
    let input = io::stdin().as_raw_handle();
    let handles = [stop.as_raw_handle(), input];
    let mut decoder = Decoder::default();
    let mut records = [MaybeUninit::<INPUT_RECORD>::uninit(); INPUT_RECORDS];
    let mut timeout = INFINITE;
    loop {
        // NOTE: Both handles remain valid for this entire wait. The stop event takes priority over unread input.
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
            // NOTE: The array has space for the requested records, and only the initialized prefix is read.
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
                // NOTE: A successful read initialized exactly `count` records.
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
}

impl Decoder {
    fn record(&mut self, record: &INPUT_RECORD) -> Option<Event> {
        match u32::from(record.EventType) {
            KEY_EVENT => {
                // NOTE: The event type identifies the union member, and ReadConsoleInputW returns UTF-16.
                let key = unsafe { record.Event.KeyEvent };
                let unit = unsafe { key.uChar.UnicodeChar };
                if unit != 0 && (key.bKeyDown != 0 || key.wVirtualKeyCode == VK_MENU) {
                    for _ in 0..key.wRepeatCount.max(1) {
                        self.push(unit);
                    }
                }
                None
            }
            WINDOW_BUFFER_SIZE_EVENT => {
                // NOTE: The event type identifies the window-size union member.
                let size = unsafe { record.Event.WindowBufferSizeEvent }.dwSize;
                Some(Event::Resize(size.X as u16, size.Y as u16))
            }
            FOCUS_EVENT => {
                // NOTE: The event type identifies the focus union member.
                let focused = unsafe { record.Event.FocusEvent }.bSetFocus != 0;
                Some(if focused {
                    Event::FocusGained
                } else {
                    Event::FocusLost
                })
            }
            MOUSE_EVENT => {
                // NOTE: The event type identifies the mouse union member.
                let mouse = unsafe { record.Event.MouseEvent };
                (mouse.dwEventFlags == MOUSE_WHEELED).then(|| {
                    let delta = (mouse.dwButtonState >> 16) as i16;
                    let kind = if delta < 0 {
                        MouseEventKind::ScrollDown
                    } else {
                        MouseEventKind::ScrollUp
                    };
                    scroll(
                        kind,
                        mouse.dwMousePosition.X as u16,
                        mouse.dwMousePosition.Y as u16,
                        KeyModifiers::NONE,
                    )
                })
            }
            _ => None,
        }
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
        self.trailing_escape = character == '\x1b';
        self.parser
            .parse(character.encode_utf8(&mut [0; 4]).as_bytes(), true);
    }

    fn finish_batch(&mut self) {
        // NOTE: A timeout disambiguates Escape. An incomplete CSI prefix may still be a paste marker.
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
                termina::event::MouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
                termina::event::MouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
                _ => return None,
            };
            Some(scroll(
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

fn scroll(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    })
}

fn stop_event() -> io::Result<OwnedHandle> {
    // NOTE: The event has default security, starts unsignaled, and its returned handle is owned here.
    let handle = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // NOTE: CreateEventW returned a valid, individually owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn mode(handle: HANDLE) -> io::Result<u32> {
    let mut mode = 0;
    // NOTE: Standard console handles are borrowed from the process. `mode` is writable for this call.
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(mode)
}

fn set_mode(handle: HANDLE, mode: u32) -> io::Result<()> {
    // NOTE: The console handle is a process standard handle, and GetConsoleMode verified it.
    if unsafe { SetConsoleMode(handle, mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::System::Console::{
        INPUT_RECORD_0, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0,
    };

    use super::{
        Decoder, Event, KeyCode, KeyEvent, KeyModifiers, INPUT_RECORD, KEY_EVENT, VK_MENU,
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
}
