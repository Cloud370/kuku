use kuku_client::{Client, Error, HistoryPage, Target};

use crate::transcript::{HistoryRequest, HISTORY_PAGE};

pub(super) async fn load(
    client: &Client,
    target: &Target,
    request: HistoryRequest,
) -> Result<HistoryPage, Error> {
    let mut page = client
        .history(target, request.before_option(), HISTORY_PAGE)
        .await?;
    let HistoryRequest::Newer { first, mut before } = request else {
        return Ok(page);
    };
    while let Some(cursor) = page.next_before.filter(|&cursor| cursor > first) {
        if cursor >= before {
            return Err(Error::Protocol("history cursor did not advance".into()));
        }
        let mut earlier = client.history(target, Some(cursor), HISTORY_PAGE).await?;
        earlier.entries.append(&mut page.entries);
        earlier.rewind_groups.append(&mut page.rewind_groups);
        page = earlier;
        before = cursor;
    }
    Ok(page)
}
