#[path = "support/terminal_state.rs"]
mod terminal_state;

use std::io::{self, Write};
use std::num::NonZeroU16;
use std::sync::{Arc, Mutex};

use terminal_state::{TerminalInput, TerminalState};

fn parser(input: TerminalInput) -> vt100::Parser<TerminalState> {
    vt100::Parser::new_with_callbacks(
        NonZeroU16::new(4).unwrap(),
        NonZeroU16::new(30).unwrap(),
        0,
        TerminalState::new(input),
    )
}

fn visible(parser: &vt100::Parser<TerminalState>) -> String {
    parser.callbacks().screen(parser.screen()).contents()
}

#[test]
fn fragmented_synchronized_updates_publish_only_the_completed_frame() {
    let mut parser = parser(TerminalInput::new(Box::new(io::sink())));
    parser.process(b"previous frame");
    for byte in b"\x1b[?2026h\x1b[2J\x1b[Hrestored reply\r\nrestored tool\x1b[?2026h" {
        parser.process(&[*byte]);
        assert_eq!(visible(&parser), "previous frame");
    }
    parser.process(b"\x1b[?202");
    assert_eq!(visible(&parser), "previous frame");
    parser.process(b"6l");
    assert_eq!(visible(&parser), "restored reply\nrestored tool");
    parser.process(b"\r\nordinary output");
    assert!(visible(&parser).ends_with("ordinary output"));
}

#[test]
fn resize_during_a_synchronized_update_keeps_the_previous_frame_until_commit() {
    let mut parser = parser(TerminalInput::new(Box::new(io::sink())));
    parser.process(b"previous frame\x1b[?2026h");
    parser.set_size(NonZeroU16::new(6).unwrap(), NonZeroU16::new(40).unwrap());
    parser.process(b"\x1b[2J\x1b[Hresized frame");
    assert_eq!(visible(&parser), "previous frame");
    assert_eq!(parser.callbacks().screen(parser.screen()).size().0.get(), 4);
    parser.process(b"\x1b[?2026l");
    assert_eq!(visible(&parser), "resized frame");
    assert_eq!(parser.callbacks().screen(parser.screen()).size().0.get(), 6);
}

#[test]
fn cursor_reports_and_input_close_work_during_a_synchronized_update() {
    #[derive(Clone)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let bytes = Buffer(Arc::new(Mutex::new(Vec::new())));
    let input = TerminalInput::new(Box::new(bytes.clone()));
    let mut parser = parser(input.clone());
    parser.process(b"\x1b[?2026h\x1b[2;3H\x1b[6n");
    assert_eq!(*bytes.0.lock().unwrap(), b"\x1b[2;3R");
    input.close();
    assert_eq!(
        input.write(b"x").unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
}
