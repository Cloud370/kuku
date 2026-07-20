import { describe, expect, it } from 'vitest';

import taskChangesJson from '../api/generated/fixtures/task_changes.json';
import taskProjectionJson from '../api/generated/fixtures/task_projection.json';
import taskStreamEventJson from '../api/generated/fixtures/task_stream_event.json';
import type {
  TaskChange,
  TaskProjection,
  TaskStreamEvent,
  TimelineItemProjection,
  TimelineWindowDelta,
} from '../api/generated';
import { decodeTaskStreamEvent } from '../api/decode';

import {
  ProjectionGapError,
  applyProjection,
  applyStreamEvent,
  beginTaskSubscription,
  createWorkbenchSnapshot,
  reduceTaskBatch,
  selectTimelineItems,
  type WorkbenchSnapshot,
} from './state';

const taskId = 'tsk_000000000000000000000001';
const otherTaskId = 'tsk_000000000000000000000002';

function fixtureProjection(): TaskProjection {
  return structuredClone(taskProjectionJson) as TaskProjection;
}

function fixtureChanges(): TaskChange[] {
  return structuredClone(taskChangesJson.changes) as TaskChange[];
}

function message(id: string, orderKey: number, text = id): TimelineItemProjection {
  return {
    type: 'message',
    item: {
      file_references: [],
      finalized: true,
      message_id: id,
      order_key: orderKey,
      request_ids: [],
      role: 'agent',
      text,
    },
  };
}

function projectionAt(
  cursor: number,
  revision: number,
  timeline: TimelineItemProjection[] = [],
): TaskProjection {
  const projection = fixtureProjection();
  projection.cursor = cursor;
  projection.task_revision = revision;
  projection.timeline = timeline;
  return projection;
}

function replacement(
  projection: TaskProjection,
  envelopeTaskId = projection.task.task_id,
): TaskStreamEvent {
  return {
    api_version: 1,
    cursor: projection.cursor,
    task_id: envelopeTaskId,
    task_revision: projection.task_revision,
    event: { type: 'projection_replaced', projection },
  };
}

function changesApplied(
  cursor: number,
  revision: number,
  changes: TaskChange[],
  timelineWindow: TimelineWindowDelta | null,
  envelopeTaskId = taskId,
): TaskStreamEvent {
  return {
    api_version: 1,
    cursor,
    task_id: envelopeTaskId,
    task_revision: revision,
    event: { type: 'changes_applied', changes, timeline_window: timelineWindow },
  };
}

function readySnapshot(projection = fixtureProjection()): WorkbenchSnapshot {
  return applyProjection(createWorkbenchSnapshot(), projection);
}

describe('task projection subscriptions', () => {
  it("requires the typed fixture stream's first frame to replace the selected Task", () => {
    const decodedFixture = decodeTaskStreamEvent(taskStreamEventJson);
    const reconnecting = beginTaskSubscription(readySnapshot(), taskId);

    expect(() => applyStreamEvent(reconnecting, decodedFixture)).toThrow(ProjectionGapError);

    const replaced = applyStreamEvent(reconnecting, replacement(projectionAt(20, 8)));
    expect(replaced.cursor).toBe(20);
    expect(replaced.connection).toBe('ready');
  });

  it('accepts non-contiguous newer cursors and same-revision activity', () => {
    const ready = readySnapshot(projectionAt(20, 8));
    const next = applyStreamEvent(
      ready,
      changesApplied(22, 8, [{ type: 'context_summary_changed', context_summary: null }], null),
    );

    expect(next.cursor).toBe(22);
    expect(next.projection?.task_revision).toBe(8);
  });

  it('rejects Task mismatch, cursor reuse or regression, and revision corruption', () => {
    const ready = readySnapshot(projectionAt(20, 8));
    const activity = [
      { type: 'context_summary_changed', context_summary: null },
    ] satisfies TaskChange[];

    expect(() =>
      applyStreamEvent(ready, changesApplied(21, 8, activity, null, otherTaskId)),
    ).toThrow(ProjectionGapError);
    expect(() => applyStreamEvent(ready, changesApplied(20, 8, activity, null))).toThrow(
      ProjectionGapError,
    );
    expect(() => applyStreamEvent(ready, changesApplied(19, 8, activity, null))).toThrow(
      ProjectionGapError,
    );
    expect(() => applyStreamEvent(ready, changesApplied(21, 7, activity, null))).toThrow(
      ProjectionGapError,
    );
    expect(() => applyStreamEvent(ready, changesApplied(21, 10, activity, null))).toThrow(
      ProjectionGapError,
    );
  });

  it('accepts an equal reconnect replacement and preserves the local draft', () => {
    const ready = readySnapshot(projectionAt(20, 8));
    ready.localDraft = { text: 'inspect', skillIds: ['skill:project:rust-review'], tierId: null };
    ready.timelineHistory.items = [message('msg_old', 1)];
    ready.timelineHistory.nextCursor = 'page:older';
    const reconnecting = beginTaskSubscription(ready, taskId);

    const replaced = applyStreamEvent(
      reconnecting,
      replacement(projectionAt(20, 8, [message('msg_new', 2)])),
    );

    expect(replaced.localDraft.text).toBe('inspect');
    expect(replaced.timelineHistory).toMatchObject({
      items: [],
      nextCursor: null,
      phase: 'idle',
      error: null,
      gapAfter: false,
    });
  });

  it('resets projection, draft, and paged history when switching Tasks', () => {
    const loaded = readySnapshot();
    loaded.localDraft.text = 'keep only for current task';
    loaded.timelineHistory.items = [message('msg_old', 1)];

    const switched = beginTaskSubscription(loaded, otherTaskId);

    expect(switched).toMatchObject({
      selectedTaskId: otherTaskId,
      projection: null,
      cursor: null,
      awaitingReplacement: true,
      connection: 'loading',
      localDraft: { text: '', skillIds: [], tierId: null },
    });
    expect(switched.timelineHistory.items).toEqual([]);
  });

  it('rejects a replacement whose envelope does not match its projection', () => {
    const waiting = beginTaskSubscription(createWorkbenchSnapshot(), taskId);
    const wrongCursor = replacement(projectionAt(9, 4));
    wrongCursor.cursor = 10;

    expect(() => applyStreamEvent(waiting, wrongCursor)).toThrow(ProjectionGapError);
    expect(() => applyStreamEvent(waiting, replacement(projectionAt(9, 4), otherTaskId))).toThrow(
      ProjectionGapError,
    );
  });

  it('rejects oversized, duplicate, or non-chronological authoritative timelines', () => {
    const waiting = beginTaskSubscription(createWorkbenchSnapshot(), taskId);
    const oversized = projectionAt(
      9,
      4,
      Array.from({ length: 501 }, (_, index) => message(`msg_${String(index)}`, index)),
    );
    const duplicate = projectionAt(9, 4, [message('same', 1), message('same', 2)]);
    const reversed = projectionAt(9, 4, [message('later', 2), message('earlier', 1)]);

    expect(() => applyStreamEvent(waiting, replacement(oversized))).toThrow(ProjectionGapError);
    expect(() => applyStreamEvent(waiting, replacement(duplicate))).toThrow(ProjectionGapError);
    expect(() => applyStreamEvent(waiting, replacement(reversed))).toThrow(ProjectionGapError);
    expect(() => applyProjection(createWorkbenchSnapshot(), oversized)).toThrow(ProjectionGapError);
  });
});

describe('atomic Task change reduction', () => {
  it('reduces all eight generated fixture change variants and stamps once', () => {
    const current = fixtureProjection();
    const reduced = reduceTaskBatch(
      current,
      createWorkbenchSnapshot().timelineHistory,
      fixtureChanges(),
      { evicted_items: [], next_cursor: null },
      1,
      8,
    );

    expect(reduced.projection).toMatchObject({
      cursor: 8,
      task_revision: 1,
      selected_tier_id: 'balanced',
      task: { state: 'completed' },
      review_summary: {
        latest_submission_id: 'rsub_000000000000000000000001',
        total_submissions: 1,
      },
    });
    expect(reduced.projection.timeline).toHaveLength(3);
    expect(reduced.projection.timeline[0]).toMatchObject({
      type: 'message',
      item: { message_id: 'msg_fixture_1', text: 'Ready. Done.', finalized: true },
    });
    expect(current).toEqual(taskProjectionJson);
  });

  it('publishes no partial state when a later change in the batch is invalid', () => {
    const ready = readySnapshot(projectionAt(20, 8));
    const before = structuredClone(ready);
    const invalid = [
      { type: 'context_summary_changed', context_summary: null },
      {
        type: 'message_patched',
        message_id: 'missing',
        append_text: 'x',
        finalized: true,
        request_ids: null,
      },
    ] satisfies TaskChange[];

    expect(() =>
      applyStreamEvent(
        ready,
        changesApplied(21, 9, invalid, { evicted_items: [], next_cursor: null }),
      ),
    ).toThrow(ProjectionGapError);
    expect(ready).toEqual(before);
  });

  it('does not retain mutable references to a batch payload', () => {
    const runChange = fixtureChanges().find((change) => change.type === 'run_state_changed');
    if (runChange?.type !== 'run_state_changed') {
      throw new Error('fixture must contain a run state change');
    }
    const originalTitle = runChange.task.title;
    const reduced = reduceTaskBatch(
      fixtureProjection(),
      createWorkbenchSnapshot().timelineHistory,
      [runChange],
      null,
      1,
      8,
    );

    runChange.task.title = 'mutated after reduction';

    expect(reduced.projection.task.title).toBe(originalTitle);
  });

  it('accepts a payload-only message patch with a null timeline window', () => {
    const reduced = reduceTaskBatch(
      projectionAt(20, 8, [message('msg_existing', 20, 'Ready')]),
      createWorkbenchSnapshot().timelineHistory,
      [
        {
          type: 'message_patched',
          message_id: 'msg_existing',
          append_text: ' now.',
          finalized: true,
          request_ids: null,
        },
      ],
      null,
      8,
      21,
    );

    expect(reduced.projection.timeline[0]).toMatchObject({
      type: 'message',
      item: { message_id: 'msg_existing', text: 'Ready now.', order_key: 20 },
    });
    expect(reduced.timelineHistory.items).toEqual([]);
  });

  it('accepts same-ID same-order activity and interaction upserts with a null window', () => {
    const changes = fixtureChanges();
    const activityChange = changes.find((change) => change.type === 'activity_upserted');
    const interactionChange = changes.find((change) => change.type === 'interaction_upserted');
    if (
      activityChange?.type !== 'activity_upserted' ||
      interactionChange?.type !== 'interaction_upserted'
    ) {
      throw new Error('fixture must contain timeline upserts');
    }
    const currentActivity = structuredClone(activityChange.activity);
    const currentInteraction = structuredClone(interactionChange.interaction);
    currentActivity.title = 'Pending read';
    currentActivity.status = 'running';
    currentInteraction.status = 'pending';
    const current = projectionAt(20, 8, [
      { type: 'activity', item: currentActivity },
      { type: 'interaction', item: currentInteraction },
    ]);

    const reduced = reduceTaskBatch(
      current,
      createWorkbenchSnapshot().timelineHistory,
      [activityChange, interactionChange],
      null,
      8,
      21,
    );

    expect(reduced.projection.timeline).toMatchObject([
      { type: 'activity', item: { title: 'Read file', status: 'completed', order_key: 9 } },
      { type: 'interaction', item: { status: 'pending', order_key: 10 } },
    ]);
    expect(reduced.timelineHistory.items).toEqual([]);
  });

  it('moves the exact chronological prefix into local history on a 500-to-501 transition', () => {
    const timeline = Array.from({ length: 500 }, (_, index) =>
      message(`msg_${String(index)}`, index),
    );
    const first = timeline[0];
    if (first === undefined) throw new Error('fixture timeline must not be empty');
    const current = projectionAt(20, 8, timeline);
    const appended = message('msg_500', 500);
    const reduced = reduceTaskBatch(
      current,
      createWorkbenchSnapshot().timelineHistory,
      [{ type: 'message_appended', item: appended }],
      { evicted_items: [first], next_cursor: 'page:older' },
      9,
      21,
    );

    expect(reduced.projection.timeline).toHaveLength(500);
    expect(reduced.projection.timeline[0]).toEqual(timeline[1]);
    expect(reduced.projection.timeline.at(-1)).toEqual(appended);
    expect(reduced.timelineHistory.items).toEqual([timeline[0]]);
    expect(reduced.timelineHistory.nextCursor).toBe('page:older');
    expect(
      selectTimelineItems({
        ...readySnapshot(reduced.projection),
        timelineHistory: reduced.timelineHistory,
      }),
    ).toHaveLength(501);
  });

  it('rejects an authoritative window that omits or invents an eviction', () => {
    const timeline = Array.from({ length: 500 }, (_, index) =>
      message(`msg_${String(index)}`, index),
    );
    const current = projectionAt(20, 8, timeline);
    const change = [
      { type: 'message_appended', item: message('msg_500', 500) },
    ] satisfies TaskChange[];

    expect(() =>
      reduceTaskBatch(
        current,
        createWorkbenchSnapshot().timelineHistory,
        change,
        { evicted_items: [], next_cursor: null },
        9,
        21,
      ),
    ).toThrow(ProjectionGapError);
    expect(() =>
      reduceTaskBatch(
        current,
        createWorkbenchSnapshot().timelineHistory,
        change,
        { evicted_items: [message('invented', -1)], next_cursor: null },
        9,
        21,
      ),
    ).toThrow(ProjectionGapError);
  });

  it('computes every eviction from one over-500 batch before publishing', () => {
    const timeline = Array.from({ length: 500 }, (_, index) =>
      message(`msg_${String(index)}`, index),
    );
    const additions = Array.from({ length: 3 }, (_, index) =>
      message(`msg_${String(500 + index)}`, 500 + index),
    );
    const changes = additions.map((item): TaskChange => ({ type: 'message_appended', item }));
    const reduced = reduceTaskBatch(
      projectionAt(20, 8, timeline),
      createWorkbenchSnapshot().timelineHistory,
      changes,
      { evicted_items: timeline.slice(0, 3), next_cursor: 'page:older' },
      9,
      21,
    );

    expect(reduced.timelineHistory.items).toEqual(timeline.slice(0, 3));
    expect(reduced.projection.timeline).toEqual([...timeline.slice(3), ...additions]);
  });

  it('accepts an authoritative byte-bound prefix below the item limit', () => {
    const first = message('msg_0', 0);
    const second = message('msg_1', 1);
    const timeline = [first, second];
    const appended = message('msg_2', 2);
    const reduced = reduceTaskBatch(
      projectionAt(20, 8, timeline),
      createWorkbenchSnapshot().timelineHistory,
      [{ type: 'message_appended', item: appended }],
      { evicted_items: [first], next_cursor: 'page:byte-bound' },
      9,
      21,
    );

    expect(reduced.projection.timeline).toEqual([second, appended]);
    expect(reduced.projection.timeline_next_cursor).toBe('page:byte-bound');
    expect(reduced.timelineHistory.items).toEqual([first]);
  });

  it('keeps the candidate when the authoritative eviction prefix is empty', () => {
    const timeline = [message('msg_0', 0)];
    const appended = message('msg_1', 1);
    const reduced = reduceTaskBatch(
      projectionAt(20, 8, timeline),
      createWorkbenchSnapshot().timelineHistory,
      [{ type: 'message_appended', item: appended }],
      { evicted_items: [], next_cursor: null },
      9,
      21,
    );

    expect(reduced.projection.timeline).toEqual([...timeline, appended]);
    expect(reduced.timelineHistory.items).toEqual([]);
  });

  it('accepts same-batch additions that are immediately evicted', () => {
    const timeline = [message('msg_1', 10), message('msg_2', 20)];
    const immediatelyEvicted = message('msg_0', 5);
    const reduced = reduceTaskBatch(
      projectionAt(20, 8, timeline),
      createWorkbenchSnapshot().timelineHistory,
      [{ type: 'message_appended', item: immediatelyEvicted }],
      { evicted_items: [immediatelyEvicted], next_cursor: 'page:byte-bound' },
      9,
      21,
    );

    expect(reduced.projection.timeline).toEqual(timeline);
    expect(reduced.timelineHistory.items).toEqual([immediatelyEvicted]);
  });

  it('rejects invented and non-prefix byte-bound evictions', () => {
    const first = message('msg_0', 0);
    const nonPrefix = message('msg_1', 1);
    const timeline = [first, nonPrefix];
    const change = [{ type: 'message_appended', item: message('msg_2', 2) }] satisfies TaskChange[];

    expect(() =>
      reduceTaskBatch(
        projectionAt(20, 8, timeline),
        createWorkbenchSnapshot().timelineHistory,
        change,
        { evicted_items: [message('invented', -1)], next_cursor: null },
        9,
        21,
      ),
    ).toThrow(ProjectionGapError);
    expect(() =>
      reduceTaskBatch(
        projectionAt(20, 8, timeline),
        createWorkbenchSnapshot().timelineHistory,
        change,
        { evicted_items: [nonPrefix], next_cursor: null },
        9,
        21,
      ),
    ).toThrow(ProjectionGapError);
  });

  it('rejects an authoritative window that evicts the entire non-empty candidate', () => {
    const appended = message('msg_0', 0);
    expect(() =>
      reduceTaskBatch(
        projectionAt(20, 8),
        createWorkbenchSnapshot().timelineHistory,
        [{ type: 'message_appended', item: appended }],
        { evicted_items: [appended], next_cursor: 'page:byte-bound' },
        9,
        21,
      ),
    ).toThrow(ProjectionGapError);
  });

  it('compares authoritative evictions structurally rather than by object key order', () => {
    const first = message('msg_0', 0, 'zero');
    if (first.type !== 'message') throw new Error('fixture item must be a message');
    const reordered: TimelineItemProjection = {
      item: {
        text: first.item.text,
        role: first.item.role,
        request_ids: first.item.request_ids,
        order_key: first.item.order_key,
        message_id: first.item.message_id,
        finalized: first.item.finalized,
        file_references: first.item.file_references,
      },
      type: 'message',
    };
    const timeline = [
      first,
      ...Array.from({ length: 499 }, (_, index) => message(`msg_${String(index + 1)}`, index + 1)),
    ];

    const reduced = reduceTaskBatch(
      projectionAt(20, 8, timeline),
      createWorkbenchSnapshot().timelineHistory,
      [{ type: 'message_appended', item: message('msg_500', 500) }],
      { evicted_items: [reordered], next_cursor: null },
      9,
      21,
    );

    expect(reduced.timelineHistory.items).toEqual([reordered]);
  });

  it('rejects inner control and Review payloads belonging to another Task', () => {
    const runChange = fixtureChanges().find((change) => change.type === 'run_state_changed');
    const reviewChange = fixtureChanges().find(
      (change) => change.type === 'review_submissions_changed',
    );
    if (
      runChange?.type !== 'run_state_changed' ||
      reviewChange?.type !== 'review_submissions_changed'
    ) {
      throw new Error('fixture must contain Task-scoped control changes');
    }
    runChange.task.task_id = otherTaskId;
    reviewChange.change.submission.task_id = otherTaskId;

    expect(() =>
      reduceTaskBatch(
        fixtureProjection(),
        createWorkbenchSnapshot().timelineHistory,
        [runChange],
        null,
        1,
        8,
      ),
    ).toThrow(ProjectionGapError);
    expect(() =>
      reduceTaskBatch(
        fixtureProjection(),
        createWorkbenchSnapshot().timelineHistory,
        [reviewChange],
        null,
        1,
        8,
      ),
    ).toThrow(ProjectionGapError);
  });

  it('requires null windows for non-timeline batches and a window for timeline batches', () => {
    const current = fixtureProjection();

    expect(() =>
      reduceTaskBatch(
        current,
        createWorkbenchSnapshot().timelineHistory,
        [{ type: 'context_summary_changed', context_summary: null }],
        { evicted_items: [], next_cursor: null },
        0,
        8,
      ),
    ).toThrow(ProjectionGapError);
    expect(() =>
      reduceTaskBatch(
        current,
        createWorkbenchSnapshot().timelineHistory,
        [{ type: 'message_appended', item: message('msg_new', 8) }],
        null,
        1,
        8,
      ),
    ).toThrow(ProjectionGapError);
  });

  it('applies an authoritative projection without retaining stale history', () => {
    const snapshot = readySnapshot();
    snapshot.timelineHistory.items = [message('stale', 1)];
    snapshot.localDraft.text = 'preserved draft';

    const next = applyProjection(snapshot, projectionAt(30, 4, [message('current', 30)]));

    expect(next.cursor).toBe(30);
    expect(next.localDraft.text).toBe('preserved draft');
    expect(next.timelineHistory.items).toEqual([]);
    expect(selectTimelineItems(next).map((item) => item.item)).toMatchObject([
      { message_id: 'current' },
    ]);
  });
});
