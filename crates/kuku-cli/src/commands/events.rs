use std::io::{self, Write};
use std::path::{Path, PathBuf};

use kuku::event::{EventStore, StoredEvent};
use kuku_client::Connection;

use crate::cli_args::{ConnectionArgs, EventsArgs};
use crate::commands::CliResult;
use crate::display::{event_conversation, render_event_brief};

const PAGE: usize = 64;

#[derive(Clone, Copy)]
enum Cursor {
    After(u64),
    Before(Option<u64>),
}

impl Cursor {
    fn at(self, id: u64) -> Self {
        match self {
            Self::After(_) => Self::After(id),
            Self::Before(_) => Self::Before(Some(id)),
        }
    }

    fn report(self) {
        match self {
            Self::After(id) => eprintln!("next page: --after {id}"),
            Self::Before(Some(id)) => eprintln!("next page: --before {id}"),
            Self::Before(None) => {}
        }
    }
}

/// Read a bounded range or stream the durable event ledger.
pub async fn run(args: EventsArgs, options: &ConnectionArgs) -> CliResult {
    let cursor = if args.all {
        Cursor::After(args.after.unwrap_or_default())
    } else {
        args.after
            .map_or(Cursor::Before(args.before), Cursor::After)
    };
    let needle = args.search.as_deref().map(str::to_lowercase);
    let mut terminal = io::BufWriter::new(io::stdout().lock());
    let mut view = View {
        args: &args,
        needle,
        cursor,
        returned: 0,
        terminal: &mut terminal,
    };
    let path = local_path(&args, options)?;
    if let Some(path) = path {
        match view.read_local(&path) {
            Ok(()) => return view.finish(),
            Err(kuku::Error::Io(error))
                if error.kind() == io::ErrorKind::WouldBlock
                    && args.file.is_none()
                    && view.returned == 0 => {}
            Err(error) => return Err(error.into()),
        }
    }

    let session = args
        .session_id
        .as_deref()
        .expect("a Session ID is required");
    view.read_service(options.connect().await?, session).await?;
    view.finish()
}

struct View<'a, W> {
    args: &'a EventsArgs,
    needle: Option<String>,
    cursor: Cursor,
    returned: usize,
    terminal: &'a mut W,
}

impl<W: Write> View<'_, W> {
    fn append(&mut self, event: &StoredEvent) -> kuku::Result<bool> {
        self.cursor = self.cursor.at(event.id);
        let args = self.args;
        if args
            .conversation
            .as_deref()
            .is_some_and(|agent| event_conversation(&event.payload).unwrap_or("main") != agent)
            || args
                .kind
                .as_deref()
                .is_some_and(|kind| event.payload.kind_name() != kind)
            || self.needle.as_deref().is_some_and(|needle| {
                !serde_json::to_string(&event.payload)
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(needle)
            })
        {
            return Ok(true);
        }
        if args.json {
            serde_json::to_writer(&mut *self.terminal, event)?;
            writeln!(self.terminal)?;
        } else {
            writeln!(self.terminal, "{}", render_event_brief(event, args.verbose))?;
        }
        self.returned += 1;
        Ok(args.all || self.returned < args.limit as usize)
    }

    fn read_local(&mut self, path: &Path) -> kuku::Result<()> {
        match self.cursor {
            Cursor::After(after) => {
                EventStore::visit_committed(path, after, |event| self.append(&event))?;
            }
            Cursor::Before(before) => {
                EventStore::visit_committed_before(path, before, |event| self.append(&event))?;
            }
        }
        Ok(())
    }

    async fn read_service(&mut self, connection: Connection, session: &str) -> CliResult {
        loop {
            let mut page = match self.cursor {
                Cursor::Before(before) => {
                    let mut page = connection
                        .client
                        .events_before(&connection.workspace, session, before, PAGE)
                        .await?;
                    page.events.reverse();
                    page
                }
                Cursor::After(after) => {
                    connection
                        .client
                        .events(&connection.workspace, session, None, after, PAGE)
                        .await?
                }
            };
            if page.events.is_empty() {
                return Ok(());
            }
            for event in page.events.drain(..) {
                if !self.append(&event)? {
                    return Ok(());
                }
            }
        }
    }

    fn finish(self) -> CliResult {
        self.terminal.flush()?;
        if !self.args.all {
            self.cursor.report();
        }
        Ok(())
    }
}

fn local_path(
    args: &EventsArgs,
    options: &ConnectionArgs,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let path = match (&args.file, &args.session_id) {
        (Some(path), _) => path.clone(),
        (_, Some(id)) if options.remote.is_none() => {
            let workspace = options.workspace()?.canonicalize()?;
            kuku::session::session_events_path(&options.data_dir()?, &workspace, id)?
        }
        _ => return Ok(None),
    };
    std::fs::metadata(&path)?;
    Ok(Some(path))
}
