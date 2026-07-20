import type {
  ApiError,
  Cursor,
  PageCursor,
  TaskChange,
  TaskProjection,
  TaskStreamEvent,
  TimelineItemProjection,
  TimelineWindowDelta,
} from '../api/generated';

const LIVE_TIMELINE_LIMIT = 500;
const HISTORY_ITEM_LIMIT = 1_500;
const HISTORY_BYTE_LIMIT = 32 * 1024 * 1024;

export interface LocalDraft {
  text: string;
  skillIds: string[];
  tierId: string | null;
}

export interface TimelineHistory {
  items: TimelineItemProjection[];
  nextCursor: PageCursor | null;
  retainedBytes: number;
  gapAfter: boolean;
  phase: 'idle' | 'loading' | 'error';
  error: ApiError | null;
  generation: number;
}

export interface WorkbenchSnapshot {
  selectedTaskId: string | null;
  projection: TaskProjection | null;
  cursor: Cursor | null;
  awaitingReplacement: boolean;
  localDraft: LocalDraft;
  timelineHistory: TimelineHistory;
  connection: 'idle' | 'loading' | 'ready' | 'reconnecting' | 'offline' | 'error';
  lastError: string | null;
}

export interface ReducedTaskBatch {
  projection: TaskProjection;
  timelineHistory: TimelineHistory;
}

export class ProjectionGapError extends Error {
  constructor(reason: string) {
    super(reason);
    this.name = 'ProjectionGapError';
  }
}

export function createWorkbenchSnapshot(): WorkbenchSnapshot {
  return {
    selectedTaskId: null,
    projection: null,
    cursor: null,
    awaitingReplacement: false,
    localDraft: emptyDraft(),
    timelineHistory: emptyTimelineHistory(),
    connection: 'idle',
    lastError: null,
  };
}

export function beginTaskSubscription(
  snapshot: WorkbenchSnapshot,
  taskId: string,
): WorkbenchSnapshot {
  const sameTask = snapshot.selectedTaskId === taskId;
  if (!sameTask) {
    return {
      ...snapshot,
      selectedTaskId: taskId,
      projection: null,
      cursor: null,
      awaitingReplacement: true,
      localDraft: emptyDraft(),
      timelineHistory: emptyTimelineHistory(snapshot.timelineHistory.generation + 1),
      connection: 'loading',
      lastError: null,
    };
  }

  return {
    ...snapshot,
    awaitingReplacement: true,
    connection: snapshot.projection === null ? 'loading' : 'reconnecting',
    lastError: null,
  };
}

export function applyProjection(
  snapshot: WorkbenchSnapshot,
  projection: TaskProjection,
): WorkbenchSnapshot {
  validateProjectionTimeline(projection);
  const taskId = projection.task.task_id;
  if (snapshot.selectedTaskId !== null && snapshot.selectedTaskId !== taskId) {
    throw new ProjectionGapError('Task projection mismatch');
  }

  const taskChanged = snapshot.selectedTaskId !== null && snapshot.selectedTaskId !== taskId;
  return {
    ...snapshot,
    selectedTaskId: taskId,
    projection: structuredClone(projection),
    cursor: projection.cursor,
    awaitingReplacement: false,
    localDraft: taskChanged ? emptyDraft() : snapshot.localDraft,
    timelineHistory: historyFromProjection(snapshot.timelineHistory, projection),
    connection: 'ready',
    lastError: null,
  };
}

export function applyStreamEvent(
  snapshot: WorkbenchSnapshot,
  event: TaskStreamEvent,
): WorkbenchSnapshot {
  if (event.task_id !== snapshot.selectedTaskId) {
    throw new ProjectionGapError('Task stream mismatch');
  }
  if (snapshot.awaitingReplacement && event.event.type !== 'projection_replaced') {
    throw new ProjectionGapError('Replacement frame required');
  }
  if (!snapshot.awaitingReplacement && event.event.type === 'projection_replaced') {
    throw new ProjectionGapError('Unexpected replacement frame');
  }
  validateCursor(snapshot, event);
  validateRevision(snapshot, event);

  const reduced =
    event.event.type === 'projection_replaced'
      ? replaceTaskProjection(
          snapshot.timelineHistory,
          event.event.projection,
          event.task_id,
          event.task_revision,
          event.cursor,
        )
      : reduceTaskBatch(
          snapshot.projection,
          snapshot.timelineHistory,
          event.event.changes,
          event.event.timeline_window,
          event.task_revision,
          event.cursor,
        );

  return {
    ...snapshot,
    projection: reduced.projection,
    cursor: event.cursor,
    timelineHistory: reduced.timelineHistory,
    awaitingReplacement: false,
    connection: 'ready',
    lastError: null,
  };
}

export function reduceTaskBatch(
  projection: TaskProjection | null,
  history: TimelineHistory,
  changes: TaskChange[],
  timelineWindow: TimelineWindowDelta | null,
  revision: number,
  cursor: Cursor,
): ReducedTaskBatch {
  if (projection === null) {
    throw new ProjectionGapError('Task changes require a projection');
  }
  validateProjectionTimeline(projection);

  const candidate = structuredClone(changes).reduce(reduceTaskChange, structuredClone(projection));
  const changesTimelineLayout = !timelineLayoutsEqual(projection.timeline, candidate.timeline);
  if (changesTimelineLayout !== (timelineWindow !== null)) {
    throw new ProjectionGapError(
      changesTimelineLayout
        ? 'Timeline change requires a window'
        : 'Timeline window requires a layout change',
    );
  }

  let nextHistory = cloneHistory(history);

  if (timelineWindow !== null) {
    const evictedCount = Math.max(0, candidate.timeline.length - LIVE_TIMELINE_LIMIT);
    const expectedEvictions = candidate.timeline.slice(0, evictedCount);
    if (!timelineItemsEqual(expectedEvictions, timelineWindow.evicted_items)) {
      throw new ProjectionGapError('Timeline window eviction mismatch');
    }

    candidate.timeline = candidate.timeline.slice(evictedCount);
    candidate.timeline_next_cursor = timelineWindow.next_cursor;
    nextHistory = appendEvictions(
      nextHistory,
      timelineWindow.evicted_items,
      timelineWindow.next_cursor,
    );
  }

  candidate.task_revision = revision;
  candidate.cursor = cursor;
  return { projection: candidate, timelineHistory: nextHistory };
}

export function selectTimelineItems(snapshot: WorkbenchSnapshot): TimelineItemProjection[] {
  const live = snapshot.projection?.timeline ?? [];
  const liveKeys = new Set(live.map(timelineItemKey));
  return [
    ...snapshot.timelineHistory.items.filter((item) => !liveKeys.has(timelineItemKey(item))),
    ...live,
  ];
}

function replaceTaskProjection(
  history: TimelineHistory,
  projection: TaskProjection,
  taskId: string,
  revision: number,
  cursor: Cursor,
): ReducedTaskBatch {
  validateProjectionTimeline(projection);
  if (
    projection.task.task_id !== taskId ||
    projection.cursor !== cursor ||
    projection.task_revision !== revision
  ) {
    throw new ProjectionGapError('Replacement envelope mismatch');
  }

  return {
    projection: structuredClone(projection),
    timelineHistory: historyFromProjection(history, projection),
  };
}

function validateCursor(snapshot: WorkbenchSnapshot, event: TaskStreamEvent): void {
  if (snapshot.cursor === null) return;
  if (event.cursor < snapshot.cursor) {
    throw new ProjectionGapError('Cursor regression');
  }
  if (!snapshot.awaitingReplacement && event.cursor === snapshot.cursor) {
    throw new ProjectionGapError('Cursor reuse');
  }
}

function validateRevision(snapshot: WorkbenchSnapshot, event: TaskStreamEvent): void {
  const currentRevision = snapshot.projection?.task_revision;
  if (currentRevision === undefined) return;
  if (event.task_revision < currentRevision) {
    throw new ProjectionGapError('Revision regression');
  }
  if (event.event.type !== 'projection_replaced' && event.task_revision > currentRevision + 1) {
    throw new ProjectionGapError('Revision jump');
  }
}

function validateProjectionTimeline(projection: TaskProjection): void {
  if (projection.timeline.length > LIVE_TIMELINE_LIMIT) {
    throw new ProjectionGapError('Task projection exceeds the live timeline limit');
  }

  const keys = new Set<string>();
  let previousOrder = Number.NEGATIVE_INFINITY;
  for (const item of projection.timeline) {
    const key = timelineItemKey(item);
    if (keys.has(key)) {
      throw new ProjectionGapError('Task projection contains a duplicate timeline item');
    }
    const order = timelineItemOrder(item);
    if (order < previousOrder) {
      throw new ProjectionGapError('Task projection timeline is not chronological');
    }
    keys.add(key);
    previousOrder = order;
  }
}

function reduceTaskChange(projection: TaskProjection, change: TaskChange): TaskProjection {
  switch (change.type) {
    case 'message_appended':
      projection.timeline = appendTimelineItem(projection.timeline, change.item);
      return projection;
    case 'message_patched':
      projection.timeline = patchMessage(projection.timeline, change);
      return projection;
    case 'activity_upserted':
      projection.timeline = upsertTimelineItem(projection.timeline, {
        type: 'activity',
        item: change.activity,
      });
      return projection;
    case 'interaction_upserted':
      projection.timeline = upsertTimelineItem(projection.timeline, {
        type: 'interaction',
        item: change.interaction,
      });
      return projection;
    case 'run_state_changed':
      if (change.task.task_id !== projection.task.task_id) {
        throw new ProjectionGapError('Run state Task mismatch');
      }
      projection.task = change.task;
      projection.active_run = change.active_run;
      projection.latest_run = change.latest_run;
      return projection;
    case 'skills_changed':
      projection.selected_tier_id = change.selected_tier_id;
      projection.loaded_skills = change.loaded_skills;
      return projection;
    case 'context_summary_changed':
      projection.context_summary = change.context_summary;
      return projection;
    case 'review_submissions_changed':
      if (change.change.submission.task_id !== projection.task.task_id) {
        throw new ProjectionGapError('Review submission Task mismatch');
      }
      projection.review_summary = {
        latest_submission_id: change.change.submission.submission_id,
        total_submissions: change.change.total_submissions,
      };
      return projection;
    default: {
      const exhaustive: never = change;
      return exhaustive;
    }
  }
}

function appendTimelineItem(
  timeline: TimelineItemProjection[],
  item: TimelineItemProjection,
): TimelineItemProjection[] {
  if (timeline.some((current) => timelineItemKey(current) === timelineItemKey(item))) {
    throw new ProjectionGapError('Timeline item already exists');
  }
  return [...timeline, item].sort(compareTimelineItems);
}

function patchMessage(
  timeline: TimelineItemProjection[],
  patch: Extract<TaskChange, { type: 'message_patched' }>,
): TimelineItemProjection[] {
  const index = timeline.findIndex(
    (item) => item.type === 'message' && item.item.message_id === patch.message_id,
  );
  if (index < 0) {
    throw new ProjectionGapError('Message patch target is missing');
  }

  return timeline.map((item, itemIndex) => {
    if (itemIndex !== index || item.type !== 'message') return item;
    return {
      ...item,
      item: {
        ...item.item,
        text: `${item.item.text}${patch.append_text}`,
        finalized: patch.finalized,
        request_ids: patch.request_ids ?? item.item.request_ids,
      },
    };
  });
}

function upsertTimelineItem(
  timeline: TimelineItemProjection[],
  item: TimelineItemProjection,
): TimelineItemProjection[] {
  const key = timelineItemKey(item);
  const withoutCurrent = timeline.filter((current) => timelineItemKey(current) !== key);
  return [...withoutCurrent, item].sort(compareTimelineItems);
}

function appendEvictions(
  history: TimelineHistory,
  evictions: TimelineItemProjection[],
  nextCursor: PageCursor | null,
): TimelineHistory {
  if (evictions.length === 0 || history.gapAfter) {
    return { ...history, nextCursor };
  }

  const existingKeys = new Set(history.items.map(timelineItemKey));
  const items = [...history.items];
  let retainedBytes = history.retainedBytes;
  let gapAfter: boolean = history.gapAfter;

  for (const item of evictions) {
    if (existingKeys.has(timelineItemKey(item))) continue;
    const bytes = timelineItemBytes(item);
    if (items.length >= HISTORY_ITEM_LIMIT || retainedBytes + bytes > HISTORY_BYTE_LIMIT) {
      gapAfter = true;
      break;
    }
    items.push(item);
    existingKeys.add(timelineItemKey(item));
    retainedBytes += bytes;
  }

  return { ...history, items, nextCursor, retainedBytes, gapAfter };
}

function historyFromProjection(
  history: TimelineHistory,
  projection: TaskProjection,
): TimelineHistory {
  return {
    ...emptyTimelineHistory(history.generation + 1),
    nextCursor: projection.timeline_next_cursor,
  };
}

function emptyDraft(): LocalDraft {
  return { text: '', skillIds: [], tierId: null };
}

function emptyTimelineHistory(generation = 0): TimelineHistory {
  return {
    items: [],
    nextCursor: null,
    retainedBytes: 0,
    gapAfter: false,
    phase: 'idle',
    error: null,
    generation,
  };
}

function cloneHistory(history: TimelineHistory): TimelineHistory {
  return { ...history, items: [...history.items] };
}

function timelineItemKey(item: TimelineItemProjection): string {
  switch (item.type) {
    case 'message':
      return `message:${item.item.message_id}`;
    case 'activity':
      return `activity:${item.item.activity_id}`;
    case 'interaction':
      return `interaction:${item.item.interaction_id}`;
  }
}

function timelineItemOrder(item: TimelineItemProjection): number {
  return item.item.order_key;
}

function compareTimelineItems(left: TimelineItemProjection, right: TimelineItemProjection): number {
  return timelineItemOrder(left) - timelineItemOrder(right);
}

function timelineItemsEqual(
  left: TimelineItemProjection[],
  right: TimelineItemProjection[],
): boolean {
  return valuesEqual(left, right);
}

function timelineLayoutsEqual(
  left: TimelineItemProjection[],
  right: TimelineItemProjection[],
): boolean {
  return (
    left.length === right.length &&
    left.every((item, index) => {
      const next = right[index];
      return (
        next !== undefined &&
        timelineItemKey(item) === timelineItemKey(next) &&
        timelineItemOrder(item) === timelineItemOrder(next)
      );
    })
  );
}

function valuesEqual(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (Array.isArray(left) || Array.isArray(right)) {
    if (!Array.isArray(left) || !Array.isArray(right) || left.length !== right.length) {
      return false;
    }
    return left.every((value, index) => valuesEqual(value, right[index]));
  }
  if (left === null || right === null || typeof left !== 'object' || typeof right !== 'object') {
    return false;
  }

  const leftRecord = left as Record<string, unknown>;
  const rightRecord = right as Record<string, unknown>;
  const leftKeys = Object.keys(leftRecord).sort();
  const rightKeys = Object.keys(rightRecord).sort();
  return (
    leftKeys.length === rightKeys.length &&
    leftKeys.every(
      (key, index) => key === rightKeys[index] && valuesEqual(leftRecord[key], rightRecord[key]),
    )
  );
}

function timelineItemBytes(item: TimelineItemProjection): number {
  return new TextEncoder().encode(JSON.stringify(item)).byteLength;
}
