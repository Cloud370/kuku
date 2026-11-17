use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use serde_json::{json, Value};

const ROWS: NonZeroU16 = NonZeroU16::new(32).unwrap();
const COLS: NonZeroU16 = NonZeroU16::new(140).unwrap();
const TOOL_TIMEOUT: Duration = Duration::from_secs(90);
const TOOL_OUTPUT: &str = "fixture-tool-output-42";

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Screen,
    Write { text: String },
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let binary = args
        .next()
        .ok_or("interactive acceptance requires an installed kuku binary")?;
    if binary == "--gated-tool" {
        return gated_tool();
    }
    if binary == "--anthropic-response" {
        let message = serde_json::from_reader(io::stdin().lock())?;
        print!("{}", kuku::test_support::anthropic_sse_response(message));
        return Ok(());
    }
    #[cfg(windows)]
    if binary == "--check-console" {
        return check_windows_console(args.next().ok_or("missing kuku binary")?, args);
    }
    host_terminal(binary, args)
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
    let mut writer = pair.master.take_writer()?;
    let viewport = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
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
                        "screen": screen.contents(),
                        "alternate_screen": screen.alternate_screen(),
                        "cursor_hidden": screen.hide_cursor(),
                        "bracketed_paste": screen.bracketed_paste(),
                        "console_restored": console_restored,
                        "exit_code": exit_code,
                    })
                }
                Request::Write { text } => {
                    writer.write_all(text.as_bytes())?;
                    writer.flush()?;
                    json!({"written": true})
                }
            };
            send(&mut stdout, &response)?;
        }
        Ok(())
    })();
    drop(writer);
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

fn gated_tool() -> Result<(), Box<dyn Error>> {
    let mut attempts = OpenOptions::new()
        .create(true)
        .append(true)
        .open("tool-attempts.txt")?;
    writeln!(attempts, "{}", env::current_dir()?.display())?;
    attempts.flush()?;
    let deadline = Instant::now() + TOOL_TIMEOUT;
    while !Path::new("release-tool").exists() {
        if Instant::now() >= deadline {
            return Err("tool release deadline expired".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    fs::write("tool-result.txt", TOOL_OUTPUT)?;
    println!("{TOOL_OUTPUT}");
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
