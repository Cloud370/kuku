use std::collections::HashSet;

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
        .then(|| {
            let projection = aggregate.projection()?;
            let history_end = aggregate
                .timeline_items()
                .len()
                .saturating_sub(projection.timeline.len());
            Ok::<_, DomainError>((
                projection,
                aggregate.timeline_items()[..history_end].to_vec(),
            ))
        })
        .transpose()?;
    let changes = aggregate.apply_record(cursor, record)?;
    let next = aggregate.projection()?;
    let layout_changed = previous
        .as_ref()
        .is_some_and(|(projection, _)| !same_timeline_layout(&projection.timeline, &next.timeline));
    let window = timeline_window(aggregate, &next, previous.as_ref());
    let timeline_window = if layout_changed || !window.evicted_items.is_empty() {
        Some(window)
    } else {
        None
    };
    Ok(TaskDelta::ChangesApplied {
        changes,
        timeline_window,
    })
}

pub fn projection(aggregate: &TaskAggregate) -> Result<TaskProjection, DomainError> {
    aggregate.projection()
}

fn timeline_window(
    aggregate: &TaskAggregate,
    next: &TaskProjection,
    previous: Option<&(TaskProjection, Vec<TimelineItemProjection>)>,
) -> TimelineWindowDelta {
    let history_end = aggregate
        .timeline_items()
        .len()
        .saturating_sub(next.timeline.len());
    let old_history = previous
        .map(|(_, history)| history.as_slice())
        .unwrap_or_default();
    let old_history: HashSet<_> = old_history.iter().map(timeline_identity).collect();
    let evicted_items = aggregate.timeline_items()[..history_end]
        .iter()
        .filter(|item| !old_history.contains(&timeline_identity(item)))
        .cloned()
        .collect();
    TimelineWindowDelta {
        next_cursor: next.timeline_next_cursor.clone(),
        evicted_items,
    }
}

fn same_timeline_layout(left: &[TimelineItemProjection], right: &[TimelineItemProjection]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| same_timeline_item(left, right))
}

fn same_timeline_item(left: &TimelineItemProjection, right: &TimelineItemProjection) -> bool {
    timeline_identity(left) == timeline_identity(right)
}

#[derive(PartialEq, Eq, Hash)]
enum TimelineIdentity<'a> {
    Message(&'a str),
    Activity(&'a str),
    Interaction(&'a kuku::event::InteractionId),
}

fn timeline_identity(item: &TimelineItemProjection) -> TimelineIdentity<'_> {
    match item {
        TimelineItemProjection::Message(item) => TimelineIdentity::Message(&item.message_id),
        TimelineItemProjection::Activity(item) => TimelineIdentity::Activity(&item.activity_id),
        TimelineItemProjection::Interaction(item) => {
            TimelineIdentity::Interaction(&item.interaction_id)
        }
    }
}
