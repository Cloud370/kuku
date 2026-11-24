use std::time::Duration;

use kuku_client::Error;
use kuku_client::local::LocalError;

use super::{Jobs, Message, Update, push};

const BUILD_RETRY_INTERVAL: Duration = Duration::from_secs(1);

impl Jobs {
    pub(super) fn connect(&mut self) {
        Self::abort(&mut self.observer);
        Self::abort(&mut self.connector);
        let options = self.options.clone();
        let build = self.build;
        let sender = self.sender.clone();
        self.connector = Some(self.tasks.spawn(async move {
            match crate::onboarding::required(&options).await {
                Ok(true) => {
                    push(&sender, Update::SetupRequired).await;
                    return;
                }
                Ok(false) => {}
                Err(error) => {
                    push(
                        &sender,
                        Update::ConnectFailed(Error::Options(error.to_string())),
                    )
                    .await;
                    return;
                }
            }
            let mut reported_wait = false;
            loop {
                let result = options.connect_for_build(build).await;
                let waiting =
                    matches!(&result, Err(Error::Local(LocalError::BuildDeferred { .. })));
                if (!waiting || !reported_wait)
                    && sender
                        .send(Message::Connection(Box::new(result)))
                        .await
                        .is_err()
                {
                    return;
                }
                if !waiting {
                    return;
                }
                reported_wait = true;
                tokio::time::sleep(BUILD_RETRY_INTERVAL).await;
            }
        }));
    }
}
