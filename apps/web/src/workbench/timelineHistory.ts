import type { ApiError, PageCursor, TimelineItemProjection } from '../api/generated';

import type { TimelineHistory } from './state';

const HISTORY_ITEM_LIMIT = 1_500;
const HISTORY_BYTE_LIMIT = 32 * 1024 * 1024;

export function prependTimelinePage(
  history: TimelineHistory,
  page: TimelineItemProjection[],
  nextCursor: PageCursor | null,
): TimelineHistory {
  const existing = new Set<string>();
  const pageItems: TimelineItemProjection[] = [];
  for (const item of page) {
    const key = timelineItemKey(item);
    if (existing.has(key)) continue;
    existing.add(key);
    pageItems.push(item);
  }

  const retained = history.items.filter((item) => {
    const key = timelineItemKey(item);
    if (existing.has(key)) return false;
    existing.add(key);
    return true;
  });
  const items = [...pageItems, ...retained].sort(compareTimelineItems);
  let retainedBytes = items.reduce((total, item) => total + timelineItemBytes(item), 0);
  let gapAfter = history.gapAfter;
  const minimumRetained = pageItems.length;

  while (
    items.length > minimumRetained &&
    (items.length > HISTORY_ITEM_LIMIT || retainedBytes > HISTORY_BYTE_LIMIT)
  ) {
    const removed = items.pop();
    if (removed === undefined) break;
    retainedBytes -= timelineItemBytes(removed);
    gapAfter = true;
  }

  return {
    ...history,
    items,
    nextCursor,
    retainedBytes,
    gapAfter,
    phase: 'idle',
    error: null,
  };
}

export function returnToRecent(
  history: TimelineHistory,
  nextCursor: PageCursor | null = null,
): TimelineHistory {
  return {
    ...history,
    items: [],
    nextCursor,
    retainedBytes: 0,
    gapAfter: false,
    phase: 'idle',
    error: null,
    generation: history.generation + 1,
  };
}

export function timelineHistoryError(
  history: TimelineHistory,
  error: ApiError | null,
): TimelineHistory {
  return { ...history, phase: 'error', error };
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

function compareTimelineItems(left: TimelineItemProjection, right: TimelineItemProjection): number {
  return left.item.order_key - right.item.order_key;
}

function timelineItemBytes(item: TimelineItemProjection): number {
  return new TextEncoder().encode(JSON.stringify(item)).byteLength;
}
