use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct TerminalInput(Arc<Mutex<Option<Box<dyn Write + Send>>>>);

impl TerminalInput {
    pub(crate) fn new(writer: Box<dyn Write + Send>) -> Self {
        Self(Arc::new(Mutex::new(Some(writer))))
    }

    pub(crate) fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut slot = self.0.lock().unwrap();
        let writer = slot
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "terminal input closed"))?;
        writer.write_all(bytes)?;
        writer.flush()
    }

    pub(crate) fn close(&self) {
        self.0.lock().unwrap().take();
    }
}

pub(crate) struct TerminalState {
    input: TerminalInput,
    previous_frame: Option<vt100::Screen>,
}

impl TerminalState {
    pub(crate) fn new(input: TerminalInput) -> Self {
        Self {
            input,
            previous_frame: None,
        }
    }

    pub(crate) fn screen<'a>(&'a self, current: &'a vt100::Screen) -> &'a vt100::Screen {
        self.previous_frame.as_ref().unwrap_or(current)
    }
}

impl vt100::Callbacks for TerminalState {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        parameters: &[&[u16]],
        command: char,
    ) {
        if (first, second) == (Some(b'?'), None)
            && parameters.iter().any(|parameter| *parameter == [2026])
        {
            match command {
                'h' if self.previous_frame.is_none() => self.previous_frame = Some(screen.clone()),
                'l' => self.previous_frame = None,
                _ => {}
            }
        } else if (first, second, command) == (None, None, 'n') && parameters == [&[6]] {
            let (row, column) = screen.cursor_position();
            let report = format!("\x1b[{};{}R", row + 1, column + 1);
            let _ = self.input.write(report.as_bytes());
        }
    }
}
