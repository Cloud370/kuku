use std::time::Duration;

use kuku_client::local::{BuildPolicy, LocalError};
use kuku_client::{Client, Connection, Error, Observation, SubscriptionUpdate};
use tokio::sync::mpsc;

use super::{Message, Selection, Update, push};
use crate::transcript::HISTORY_PAGE;

const RECONNECT_DELAY: Duration = Duration::from_secs(2);

pub(super) async fn watch(
    connection: Connection,
    build: BuildPolicy<'_>,
    selection: Selection,
    sender: mpsc::Sender<Message>,
) {
    loop {
        let error = observe(&connection.client, &selection, &sender).await;
        if let Some(service) = &connection.local {
            if let Err(error @ LocalError::BuildMismatch { .. }) =
                service.check_running_build(build).await
            {
                push(&sender, Update::WatchFailed(selection, error.into())).await;
                return;
            }
        }
        if !matches!(
            error,
            Error::Transport(_)
                | Error::ObservationEnded
                | Error::HttpStatus(502..=504)
                | Error::Service {
                    status: 502..=504,
                    ..
                }
        ) {
            push(&sender, Update::WatchFailed(selection, error)).await;
            return;
        }
        if !push(
            &sender,
            Update::WatchReconnecting(selection.clone(), error.to_string()),
        )
        .await
        {
            return;
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

pub(super) async fn watch_workspace_permissions(
    client: Client,
    selection: Selection,
    sender: mpsc::Sender<Message>,
) {
    let mut subscription = client.subscribe_workspace_permissions(&selection.target);
    while let Ok(observation) = subscription.next().await {
        if matches!(observation, Observation::WorkspacePermission { .. })
            && !push(&sender, Update::Observation(selection.clone(), observation)).await
        {
            return;
        }
    }
}

async fn observe(client: &Client, selection: &Selection, sender: &mpsc::Sender<Message>) -> Error {
    let target = &selection.target;
    let snapshot = match client.observation_snapshot(target).await {
        Ok(snapshot) => snapshot,
        Err(error) => return error,
    };
    let page = match client
        .history(
            target,
            Some(snapshot.cursor.saturating_add(1)),
            HISTORY_PAGE,
        )
        .await
    {
        Ok(page) => page,
        Err(error) => return error,
    };
    let mut subscription = client.subscribe_with_agent_activity(target, snapshot.cursor);
    if !push(
        sender,
        Update::Loaded(selection.clone(), Box::new(snapshot), page),
    )
    .await
    {
        return Error::ObservationEnded;
    }
    loop {
        let update = match subscription.next_update().await {
            Ok(SubscriptionUpdate::Connected) => Update::WatchConnected(selection.clone()),
            Ok(SubscriptionUpdate::Reconnecting(error)) => {
                Update::WatchReconnecting(selection.clone(), error)
            }
            Ok(SubscriptionUpdate::Observation(event)) => {
                Update::Observation(selection.clone(), event)
            }
            Err(error) => return error,
        };
        if !push(sender, update).await {
            return Error::ObservationEnded;
        }
    }
}
