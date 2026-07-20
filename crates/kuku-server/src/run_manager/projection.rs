use kuku::event::TaskLedgerRecord;

use crate::api::{TaskDelta, TaskProjection, TimelineItemProjection, TimelineWindowDelta};

use super::{DomainError, TaskAggregate};

pub fn reduce_record(
    aggregate: &mut TaskAggregate,
    cursor: kuku::event::Cursor,
    record: &TaskLedgerRecord,
) -> Result<TaskDelta, DomainError> {
    let previous = aggregate
        .task_id()
        .is_some()
        .then(|| aggregate.projection())
        .transpose()?;
    let changes = aggregate.apply_record(cursor, record)?;
    let next = aggregate.projection()?;
    let timeline_window = previous.and_then(|previous| timeline_window(&previous, &next));
    Ok(TaskDelta::ChangesApplied {
        changes,
        timeline_window,
    })
}

pub fn projection(aggregate: &TaskAggregate) -> Result<TaskProjection, DomainError> {
    aggregate.projection()
}

fn timeline_window(
    previous: &TaskProjection,
    next: &TaskProjection,
) -> Option<TimelineWindowDelta> {
    let evicted_items: Vec<_> = previous
        .timeline
        .iter()
        .filter(|old| !next.timeline.iter().any(|new| same_timeline_item(old, new)))
        .cloned()
        .collect();
    if evicted_items.is_empty() && previous.timeline_next_cursor == next.timeline_next_cursor {
        return None;
    }
    Some(TimelineWindowDelta {
        next_cursor: next.timeline_next_cursor.clone(),
        evicted_items,
    })
}

fn same_timeline_item(left: &TimelineItemProjection, right: &TimelineItemProjection) -> bool {
    match (left, right) {
        (TimelineItemProjection::Message(left), TimelineItemProjection::Message(right)) => {
            left.message_id == right.message_id
        }
        (TimelineItemProjection::Activity(left), TimelineItemProjection::Activity(right)) => {
            left.activity_id == right.activity_id
        }
        (TimelineItemProjection::Interaction(left), TimelineItemProjection::Interaction(right)) => {
            left.interaction_id == right.interaction_id
        }
        _ => false,
    }
}
