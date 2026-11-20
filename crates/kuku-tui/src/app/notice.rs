use std::time::{Duration, Instant};

use super::{Address, App};

#[derive(Debug)]
enum NoticeLifetime {
    Persistent,
    Transient(Instant),
    ExecutionStart { address: Address, after: u64 },
}

#[derive(Debug)]
pub(crate) struct Notice {
    pub(crate) text: String,
    pub(crate) error: bool,
    lifetime: NoticeLifetime,
}

impl Notice {
    pub(crate) fn transient(&self) -> bool {
        matches!(&self.lifetime, NoticeLifetime::Transient(_))
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        match &self.lifetime {
            NoticeLifetime::Transient(deadline) => Some(*deadline),
            NoticeLifetime::Persistent | NoticeLifetime::ExecutionStart { .. } => None,
        }
    }

    fn clears_for_execution_start(&self, address: &Address, execution_id: u64) -> bool {
        matches!(
            &self.lifetime,
            NoticeLifetime::ExecutionStart {
                address: notice_address,
                after,
            } if notice_address == address && execution_id > *after
        )
    }
}

impl App {
    pub(crate) fn message(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            error: false,
            lifetime: NoticeLifetime::Persistent,
        });
    }

    pub(crate) fn confirmation(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            error: false,
            lifetime: NoticeLifetime::Transient(Instant::now() + Duration::from_secs(4)),
        });
    }

    pub(crate) fn problem(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            error: true,
            lifetime: NoticeLifetime::Persistent,
        });
    }

    pub(crate) fn stopped_execution_notice(&mut self, address: Address, execution_id: u64) {
        self.notice = Some(Notice {
            text: "Execution stopped · draft kept".into(),
            error: false,
            lifetime: NoticeLifetime::ExecutionStart {
                address,
                after: execution_id,
            },
        });
    }

    pub(crate) fn clear_execution_notice_on_start(&mut self, address: &Address, execution_id: u64) {
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| notice.clears_for_execution_start(address, execution_id))
        {
            self.notice = None;
        }
    }
}
