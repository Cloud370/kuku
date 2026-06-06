use crate::error::{Error, Result};

use super::{BufferedLogWriter, HostKind, LogRecord, LogScope};

pub struct LogHub {
    session: Option<BufferedLogWriter>,
    runtime: Option<BufferedLogWriter>,
    host_cli: Option<BufferedLogWriter>,
    host_server: Option<BufferedLogWriter>,
    host_webui: Option<BufferedLogWriter>,
}

impl LogHub {
    pub fn new(
        session: Option<BufferedLogWriter>,
        runtime: Option<BufferedLogWriter>,
        host_cli: Option<BufferedLogWriter>,
        host_server: Option<BufferedLogWriter>,
        host_webui: Option<BufferedLogWriter>,
    ) -> Self {
        Self {
            session,
            runtime,
            host_cli,
            host_server,
            host_webui,
        }
    }

    pub fn write(&mut self, record: LogRecord) -> Result<()> {
        match record.scope {
            LogScope::Session => self
                .session
                .as_mut()
                .ok_or_else(|| {
                    Error::InvalidArgument("session log writer is not configured".into())
                })?
                .push(record),
            LogScope::Runtime => self
                .runtime
                .as_mut()
                .ok_or_else(|| {
                    Error::InvalidArgument("runtime log writer is not configured".into())
                })?
                .push(record),
            LogScope::Host => match record.host {
                Some(HostKind::Cli) => self
                    .host_cli
                    .as_mut()
                    .ok_or_else(|| {
                        Error::InvalidArgument("cli log writer is not configured".into())
                    })?
                    .push(record),
                Some(HostKind::Server) => self
                    .host_server
                    .as_mut()
                    .ok_or_else(|| {
                        Error::InvalidArgument("server log writer is not configured".into())
                    })?
                    .push(record),
                Some(HostKind::Webui) => self
                    .host_webui
                    .as_mut()
                    .ok_or_else(|| {
                        Error::InvalidArgument("webui log writer is not configured".into())
                    })?
                    .push(record),
                None => Err(Error::InvalidArgument(
                    "host log record must include a host kind".into(),
                )),
            },
        }
    }

    pub fn flush(&mut self) -> Result<()> {
        if let Some(writer) = self.session.as_mut() {
            writer.flush()?;
        }
        if let Some(writer) = self.runtime.as_mut() {
            writer.flush()?;
        }
        if let Some(writer) = self.host_cli.as_mut() {
            writer.flush()?;
        }
        if let Some(writer) = self.host_server.as_mut() {
            writer.flush()?;
        }
        if let Some(writer) = self.host_webui.as_mut() {
            writer.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{LogLevel, LogScope};

    #[test]
    fn host_records_require_host_kind() {
        let mut hub = LogHub::new(None, None, None, None, None);
        let mut record = LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Host);
        record.kind = "host.ready".to_string();
        record.message = "ready".to_string();

        let error = hub.write(record).unwrap_err();
        assert!(error.to_string().contains("host kind"));
    }
}
