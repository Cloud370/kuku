use kuku::event::TaskLedgerRecord;

use crate::api::{TaskDelta, TaskProjection};

use super::{DomainError, TaskAggregate};

pub fn reduce_record(
    aggregate: &mut TaskAggregate,
    cursor: kuku::event::Cursor,
    record: &TaskLedgerRecord,
) -> Result<TaskDelta, DomainError> {
    let changes = aggregate.apply_record(cursor, record)?;
    if changes.is_empty() {
        return Ok(TaskDelta::ProjectionReplaced {
            projection: Box::new(aggregate.projection()?),
        });
    }
    Ok(TaskDelta::ChangesApplied {
        changes,
        timeline_window: None,
    })
}

pub fn projection(aggregate: &TaskAggregate) -> Result<TaskProjection, DomainError> {
    aggregate.projection()
}
