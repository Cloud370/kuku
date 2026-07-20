use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tempfile::tempdir;

use kuku::event::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, EventPayload, MessageFact, MessageRoleFact,
    RunId, TaskActivityBatch, TaskEvent, TaskId, TaskLedgerRecord, TaskRevision, WorkspaceId,
};

use super::driver::{DriverHandle, DriverStart, RunDriverFactory};
use super::store::CreateTaskCommand;
use super::{DomainError, TaskRuntime};

#[derive(Default)]
struct NoopFactory;

impl RunDriverFactory for NoopFactory {
    fn start(
        &self,
        _start: DriverStart,
    ) -> Pin<Box<dyn Future<Output = Result<DriverHandle, DomainError>> + Send>> {
        Box::pin(async { Err(DomainError::RunNotActive) })
    }
}

fn workspace_id() -> WorkspaceId {
    WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap()
}

async fn fixture() -> (TaskRuntime, TaskId) {
    let home = tempdir().unwrap().keep();
    let runtime = TaskRuntime::open_unchecked(home, Arc::new(NoopFactory), 1, 1).unwrap();
    let created = runtime
        .create_task(CreateTaskCommand {
            workspace_id: workspace_id(),
            idempotency_key: "subscription-create".to_owned(),
        })
        .await
        .unwrap();
    (runtime, created.projection.task.task_id)
}

fn activity(id: &str) -> TaskLedgerRecord {
    TaskLedgerRecord::Activity(
        TaskActivityBatch::try_new(vec![TaskEvent::ActivityUpserted {
            activity: ActivityFact {
                activity_id: id.to_owned(),
                run_id: RunId::parse("run_0123456789abcdef01234567").unwrap(),
                title: id.to_owned(),
                kind: ActivityKindFact::System,
                status: ActivityStatusFact::Completed,
                detail: None,
                conversation_id: None,
                agent: None,
                tier: None,
                result_in_main: None,
                file_references: Vec::new(),
            },
        }])
        .unwrap(),
    )
}

fn messages(task_id: &TaskId, range: std::ops::Range<usize>) -> TaskLedgerRecord {
    let revision = range.start as u64 / 500 + 1;
    let key = format!("messages-{}-{}", range.start, range.end);
    control_record(
        revision,
        &key,
        range
            .map(|index| TaskEvent::MessageAppended {
                message: MessageFact {
                    message_id: format!("message-{index}"),
                    task_id: task_id.clone(),
                    run_id: None,
                    role: MessageRoleFact::Agent,
                    text: index.to_string(),
                    finalized: true,
                    request_ids: Vec::new(),
                    file_references: Vec::new(),
                },
            })
            .collect(),
    )
}

fn control_record(revision: u64, key: &str, events: Vec<TaskEvent>) -> TaskLedgerRecord {
    TaskLedgerRecord::Control(
        kuku::event::TaskTransaction::try_new(
            TaskRevision::try_new(revision).unwrap(),
            kuku::event::CommandReceipt::new(key, key, kuku::event::CommandResult::Stopped)
                .unwrap(),
            events,
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn subscribe_starts_with_replacement_and_incremental_delta() {
    let (runtime, task_id) = fixture().await;
    let mut subscription = runtime.subscribe(&task_id, None).await.unwrap();
    let replacement = subscription.next().await.unwrap();
    assert!(matches!(
        replacement.event,
        crate::api::TaskDelta::ProjectionReplaced { .. }
    ));

    runtime
        .repository()
        .append(&task_id, activity("one"))
        .unwrap();
    let delta = subscription.next().await.unwrap();
    assert!(matches!(
        delta.event,
        crate::api::TaskDelta::ChangesApplied { changes, .. }
            if matches!(changes.as_slice(), [crate::api::TaskChange::ActivityUpserted { .. }])
    ));
    assert!(delta.cursor > replacement.cursor);
}

#[tokio::test]
async fn subscribe_rejects_cursor_ahead() {
    let (runtime, task_id) = fixture().await;
    let current = runtime.projection(&task_id).await.unwrap().cursor;
    let ahead = kuku::event::Cursor::try_new(current.get() + 1).unwrap();
    assert_eq!(
        runtime.subscribe(&task_id, Some(ahead)).await.unwrap_err(),
        DomainError::CursorAhead
    );
}

#[tokio::test]
async fn reconnect_replaces_then_delivers_only_newer_deltas() {
    let (runtime, task_id) = fixture().await;
    let mut first = runtime.subscribe(&task_id, None).await.unwrap();
    let replacement = first.next().await.unwrap();
    drop(first);
    runtime
        .repository()
        .append(&task_id, activity("one"))
        .unwrap();
    runtime
        .repository()
        .append(&task_id, activity("two"))
        .unwrap();

    let mut resumed = runtime
        .subscribe(&task_id, Some(replacement.cursor))
        .await
        .unwrap();
    let resumed_replacement = resumed.next().await.unwrap();
    assert!(matches!(
        resumed_replacement.event,
        crate::api::TaskDelta::ProjectionReplaced { .. }
    ));
    runtime
        .repository()
        .append(&task_id, activity("three"))
        .unwrap();
    let delta = resumed.next().await.unwrap();
    assert!(delta.cursor > resumed_replacement.cursor);
    assert!(matches!(
        delta.event,
        crate::api::TaskDelta::ChangesApplied { changes, .. }
            if matches!(changes.as_slice(), [crate::api::TaskChange::ActivityUpserted { .. }])
    ));
}

#[tokio::test]
async fn sdk_only_cursors_are_skipped_without_requiring_contiguity() {
    let (runtime, task_id) = fixture().await;
    let mut subscription = runtime.subscribe(&task_id, None).await.unwrap();
    let replacement = subscription.next().await.unwrap();
    let mut store = runtime.repository().event_store(&task_id).unwrap();
    let sdk_event = store
        .append_synced(EventPayload::ConversationOpened {
            ts: "2026-07-20T00:00:00Z".to_owned(),
            conversation: "main".to_owned(),
        })
        .unwrap();
    runtime
        .repository()
        .append(&task_id, activity("after-sdk"))
        .unwrap();
    let delta = subscription.next().await.unwrap();
    assert!(delta.cursor.get() > sdk_event.id);
    assert!(delta.cursor > replacement.cursor);
}

#[tokio::test]
async fn lag_recovery_emits_one_current_replacement() {
    let (runtime, task_id) = fixture().await;
    let mut subscription = runtime.subscribe(&task_id, None).await.unwrap();
    let initial = subscription.next().await.unwrap();
    for index in 0..300 {
        runtime
            .repository()
            .append(&task_id, activity(&format!("activity-{index}")))
            .unwrap();
    }
    let replacement = subscription.next().await.unwrap();
    assert!(matches!(
        replacement.event,
        crate::api::TaskDelta::ProjectionReplaced { .. }
    ));
    assert!(replacement.cursor > initial.cursor);
    runtime
        .repository()
        .append(&task_id, activity("after-lag"))
        .unwrap();
    let next = subscription.next().await.unwrap();
    assert!(
        matches!(next.event, crate::api::TaskDelta::ChangesApplied { .. }),
        "{next:?}"
    );
    assert!(next.cursor > replacement.cursor);
}

#[tokio::test]
async fn live_window_metadata_matches_replay_at_five_hundred_to_five_hundred_one() {
    let (runtime, task_id) = fixture().await;
    runtime
        .repository()
        .append(&task_id, messages(&task_id, 0..500))
        .unwrap();
    let mut subscription = runtime.subscribe(&task_id, None).await.unwrap();
    let replacement = subscription.next().await.unwrap();
    let crate::api::TaskDelta::ProjectionReplaced { projection } = replacement.event else {
        panic!("subscription must start with replacement")
    };

    runtime
        .repository()
        .append(&task_id, messages(&task_id, 500..501))
        .unwrap();
    let live = subscription.next().await.unwrap();
    let crate::api::TaskDelta::ChangesApplied {
        changes,
        timeline_window: Some(window),
    } = live.event
    else {
        panic!("timeline append must include a window")
    };
    assert_eq!(changes.len(), 1);
    assert_eq!(window.evicted_items, projection.timeline[..1]);
    let replayed = runtime.repository().rebuild(&task_id).unwrap();
    let current = runtime.projection(&task_id).await.unwrap();
    let combined = window
        .evicted_items
        .iter()
        .chain(current.timeline.iter())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(combined, replayed.timeline_items());
}

#[tokio::test]
async fn one_large_batch_evicts_the_exact_untrimmed_prefix_in_one_frame() {
    let (runtime, task_id) = fixture().await;
    runtime
        .repository()
        .append(&task_id, messages(&task_id, 0..500))
        .unwrap();
    let mut subscription = runtime.subscribe(&task_id, None).await.unwrap();
    let initial = subscription.next().await.unwrap();
    let crate::api::TaskDelta::ProjectionReplaced { projection } = initial.event else {
        panic!("subscription must start with replacement")
    };

    runtime
        .repository()
        .append(&task_id, messages(&task_id, 500..1100))
        .unwrap();
    let live = subscription.next().await.unwrap();
    let crate::api::TaskDelta::ChangesApplied {
        changes,
        timeline_window: Some(window),
    } = live.event
    else {
        panic!("large append must include a window")
    };
    assert_eq!(changes.len(), 600);
    let mut candidate = projection.timeline.clone();
    candidate.extend(changes.iter().map(|change| match change {
        crate::api::TaskChange::MessageAppended { item } => item.clone(),
        _ => panic!("fixture only appends messages"),
    }));
    assert_eq!(window.evicted_items, candidate[..600]);
    let current = runtime.projection(&task_id).await.unwrap();
    assert_eq!(current.timeline, candidate[600..]);
    let replayed = runtime.repository().rebuild(&task_id).unwrap();
    let combined = window
        .evicted_items
        .iter()
        .chain(current.timeline.iter())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(combined, replayed.timeline_items());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), subscription.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn message_patch_is_one_coalesced_frame_without_window_metadata() {
    let (runtime, task_id) = fixture().await;
    runtime
        .repository()
        .append(&task_id, messages(&task_id, 0..1))
        .unwrap();
    let mut subscription = runtime.subscribe(&task_id, None).await.unwrap();
    subscription.next().await.unwrap();
    let patch = TaskLedgerRecord::Activity(
        TaskActivityBatch::try_new(vec![TaskEvent::MessagePatched {
            message_id: "message-0".to_owned(),
            append_text: " suffix".to_owned(),
            finalized: true,
            request_ids: None,
        }])
        .unwrap(),
    );
    runtime.repository().append(&task_id, patch).unwrap();
    let frame = subscription.next().await.unwrap();
    assert!(matches!(
        frame.event,
        crate::api::TaskDelta::ChangesApplied {
            changes,
            timeline_window: None,
        } if matches!(changes.as_slice(), [crate::api::TaskChange::MessagePatched { append_text, .. }] if append_text == " suffix")
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), subscription.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn per_task_stream_limit_is_eight_and_drop_releases_slot() {
    let (runtime, task_id) = fixture().await;
    let mut subscriptions = Vec::new();
    for _ in 0..8 {
        subscriptions.push(runtime.subscribe(&task_id, None).await.unwrap());
    }
    assert_eq!(
        runtime.subscribe(&task_id, None).await.unwrap_err(),
        DomainError::StreamLimit
    );
    drop(subscriptions.pop());
    assert!(runtime.subscribe(&task_id, None).await.is_ok());
}

#[tokio::test]
async fn total_stream_limit_is_sixty_four() {
    let (runtime, first_task) = fixture().await;
    let mut task_ids = vec![first_task];
    for index in 1..9 {
        let created = runtime
            .create_task(CreateTaskCommand {
                workspace_id: workspace_id(),
                idempotency_key: format!("subscription-create-{index}"),
            })
            .await
            .unwrap();
        task_ids.push(created.projection.task.task_id);
    }
    let mut subscriptions = Vec::new();
    for task_id in &task_ids[..8] {
        for _ in 0..8 {
            subscriptions.push(runtime.subscribe(task_id, None).await.unwrap());
        }
    }
    assert_eq!(
        runtime.subscribe(&task_ids[8], None).await.unwrap_err(),
        DomainError::StreamLimit
    );
    drop(subscriptions.pop());
    assert!(runtime.subscribe(&task_ids[8], None).await.is_ok());
}

#[tokio::test]
async fn missing_task_is_not_reported_as_a_storage_error() {
    let (runtime, _) = fixture().await;
    let missing = TaskId::parse("tsk_1123456789abcdef01234567").unwrap();
    assert_eq!(
        runtime.subscribe(&missing, None).await.unwrap_err(),
        DomainError::TaskNotFound
    );
}
