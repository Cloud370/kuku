use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::io::{self, BufRead, Read, Write};
use std::num::NonZeroU16;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use serde_json::{json, Value};

const ROWS: NonZeroU16 = NonZeroU16::new(32).unwrap();
const COLS: NonZeroU16 = NonZeroU16::new(140).unwrap();
const TOOL_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Screen,
    Resize { rows: NonZeroU16, cols: NonZeroU16 },
    Write { text: String },
}

#[derive(Clone)]
struct TerminalInput(Arc<Mutex<Option<Box<dyn Write + Send>>>>);

impl TerminalInput {
    fn new(writer: Box<dyn Write + Send>) -> Self {
        Self(Arc::new(Mutex::new(Some(writer))))
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut slot = self.0.lock().unwrap();
        let writer = slot
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "terminal input closed"))?;
        writer.write_all(bytes)?;
        writer.flush()
    }

    fn close(&self) {
        self.0.lock().unwrap().take();
    }
}

impl vt100::Callbacks for TerminalInput {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        parameters: &[&[u16]],
        command: char,
    ) {
        // NOTE: ConPTY inherits the cursor via a report from its host terminal before the child can start.
        if (first, second, command) == (None, None, 'n') && parameters == [&[6]] {
            let (row, column) = screen.cursor_position();
            let report = format!("\x1b[{};{}R", row + 1, column + 1);
            let _ = self.write(report.as_bytes());
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let binary = args
        .next()
        .ok_or("interactive acceptance requires an installed kuku binary")?;
    if binary == "--gated-tool" {
        kuku::test_support::run_gated_tool(TOOL_TIMEOUT)?;
        return Ok(());
    }
    if binary == "--anthropic-response" {
        let message = serde_json::from_reader(io::stdin().lock())?;
        print!("{}", kuku::test_support::anthropic_sse_response(message));
        return Ok(());
    }
    if binary == "--workspace-log" {
        workspace_log()?;
        std::process::exit(7);
    }
    #[cfg(windows)]
    if binary == "--check-console" {
        return check_windows_console(args.next().ok_or("missing kuku binary")?, args);
    }
    host_terminal(binary, args)
}

fn workspace_log() -> io::Result<()> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in 1..=8193 {
        let (level, code, component) = match line {
            23 | 7103 => ("ERROR", "E_SCHEMA", "入口"),
            3119 => ("ERROR", "E_DISK", "store"),
            8193 => ("ERROR", "E_TIMEOUT", "worker"),
            _ => ("INFO", "OK", "worker"),
        };
        writeln!(
            output,
            "{{\"line\":{line},\"level\":\"{level}\",\"code\":\"{code}\",\"component\":\"{component}\",\"message\":\"Processed input and recorded its delivery outcome\"}}"
        )?;
    }
    output.flush()?;
    writeln!(
        io::stderr().lock(),
        "fixture replay completed with rejected records"
    )
}

fn host_terminal(
    binary: OsString,
    args: impl Iterator<Item = OsString>,
) -> Result<(), Box<dyn Error>> {
    let size = PtySize {
        rows: ROWS.get(),
        cols: COLS.get(),
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = native_pty_system().openpty(size)?;
    #[cfg(unix)]
    let before = pair.master.get_termios();
    let mut command = terminal_command(binary)?;
    command.args(args);
    command.cwd(env::current_dir()?);
    command.env("TERM", "xterm-256color");

    let mut reader = pair.master.try_clone_reader()?;
    let input = TerminalInput::new(pair.master.take_writer()?);
    let viewport = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
        ROWS,
        COLS,
        0,
        input.clone(),
    )));
    let parser = viewport.clone();
    thread::spawn(move || {
        let mut bytes = [0; 4096];
        while let Ok(size @ 1..) = reader.read(&mut bytes) {
            parser.lock().unwrap().process(&bytes[..size]);
        }
    });
    let mut child = pair.slave.spawn_command(command)?;
    let mut stdout = io::stdout().lock();
    let result = (|| {
        send(
            &mut stdout,
            &json!({"backend": if cfg!(windows) { "conpty" } else { "unix-pty" }}),
        )?;
        for line in io::stdin().lock().lines() {
            let response = match serde_json::from_str::<Request>(&line?)? {
                Request::Screen => {
                    let exit_code = child.try_wait()?.map(|status| status.exit_code());
                    let parser = viewport.lock().unwrap();
                    let screen = parser.screen();
                    #[cfg(unix)]
                    let console_restored = before.is_some() && before == pair.master.get_termios();
                    #[cfg(windows)]
                    let console_restored = exit_code == Some(0);
                    json!({
                        "screen": screen.rows(0, screen.size().1.get()).collect::<Vec<_>>().join("\n"),
                        "alternate_screen": screen.alternate_screen(),
                        "cursor_hidden": screen.hide_cursor(),
                        "bracketed_paste": screen.bracketed_paste(),
                        "console_restored": console_restored,
                        "exit_code": exit_code,
                    })
                }
                Request::Resize { rows, cols } => {
                    viewport.lock().unwrap().set_size(rows, cols);
                    pair.master.resize(PtySize {
                        rows: rows.get(),
                        cols: cols.get(),
                        ..size
                    })?;
                    json!({"rows": rows, "cols": cols})
                }
                Request::Write { text } => {
                    input.write(text.as_bytes())?;
                    json!({"written": true})
                }
            };
            send(&mut stdout, &response)?;
        }
        Ok(())
    })();
    input.close();
    drop(pair);
    if child.try_wait()?.is_none() {
        let _ = child.kill();
        child.wait()?;
    }
    result
}

fn terminal_command(binary: OsString) -> io::Result<CommandBuilder> {
    #[cfg(windows)]
    {
        let mut command = CommandBuilder::new(env::current_exe()?);
        command.arg("--check-console");
        command.arg(binary);
        Ok(command)
    }
    #[cfg(unix)]
    {
        Ok(CommandBuilder::new(binary))
    }
}

fn send(writer: &mut impl Write, message: &Value) -> Result<(), Box<dyn Error>> {
    serde_json::to_writer(&mut *writer, message)?;
    writeln!(writer)?;
    writer.flush()?;
    Ok(())
}

#[cfg(windows)]
fn check_windows_console(
    binary: OsString,
    args: impl Iterator<Item = OsString>,
) -> Result<(), Box<dyn Error>> {
    use std::process::Command;

    let before = windows_input_mode()?;
    let status = Command::new(binary).args(args).status()?;
    let after = windows_input_mode()?;
    if before != after {
        return Err(format!("Console input mode changed from {before:#x} to {after:#x}").into());
    }
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(windows)]
fn windows_input_mode() -> io::Result<u32> {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE};

    let mut mode = 0;
    // NOTE: std::io does not expose console mode flags. The OS owns the handle and mode is writable for this call.
    let ok = unsafe { GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(mode)
}
