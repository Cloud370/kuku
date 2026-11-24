use kuku_client::{Client, Error, ForwardHistoryPage, HistoryPage, Target};

use crate::transcript::{HISTORY_PAGE, HistoryRequest};

pub(crate) enum LoadedHistory {
    Before(HistoryPage),
    After(ForwardHistoryPage),
}

impl From<HistoryPage> for LoadedHistory {
    fn from(page: HistoryPage) -> Self {
        Self::Before(page)
    }
}

pub(super) async fn load(
    client: &Client,
    target: &Target,
    request: HistoryRequest,
) -> Result<LoadedHistory, Error> {
    match request {
        HistoryRequest::Newer(after) => {
            let page = client.history_after(target, after, HISTORY_PAGE).await?;
            if page.next_after.is_some_and(|cursor| cursor <= after) {
                return Err(Error::Protocol("history cursor did not advance".into()));
            }
            Ok(LoadedHistory::After(page))
        }
        request => Ok(client
            .history(target, request.before_option(), HISTORY_PAGE)
            .await?
            .into()),
    }
}
