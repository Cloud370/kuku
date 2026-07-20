import { describe, expect, it } from 'vitest';

import type { TimelineItemProjection } from '../api/generated';

import { createWorkbenchSnapshot } from './state';
import { prependTimelinePage, returnToRecent } from './timelineHistory';

function message(number: number, text = `message ${String(number)}`): TimelineItemProjection {
  return {
    type: 'message',
    item: {
      file_references: [],
      finalized: true,
      message_id: `msg_${String(number)}`,
      order_key: number,
      request_ids: [],
      role: 'agent',
      text,
    },
  };
}

describe('timeline history paging', () => {
  it('prepends chronological pages, deduplicates stable IDs, and advances the opaque cursor', () => {
    const history = createWorkbenchSnapshot().timelineHistory;
    history.items = [message(3), message(4)];
    history.nextCursor = 'page:first';

    const next = prependTimelinePage(history, [message(1), message(2), message(3)], 'page:next');

    expect(next.items.map((item) => item.item.order_key)).toEqual([1, 2, 3, 4]);
    expect(next.nextCursor).toBe('page:next');
    expect(next.phase).toBe('idle');
    expect(next.error).toBeNull();
  });

  it('preserves the newly loaded page and marks a gap when the older-item bound is crossed', () => {
    const history = createWorkbenchSnapshot().timelineHistory;
    history.items = Array.from({ length: 1_500 }, (_, index) => message(index + 101));
    history.retainedBytes = history.items.reduce(
      (total, item) => total + new TextEncoder().encode(JSON.stringify(item)).byteLength,
      0,
    );

    const next = prependTimelinePage(
      history,
      Array.from({ length: 100 }, (_, index) => message(index + 1)),
      'page:older',
    );

    expect(next.items).toHaveLength(1_500);
    expect(next.items[0]?.item.order_key).toBe(1);
    expect(next.items.at(-1)?.item.order_key).toBe(1_500);
    expect(next.gapAfter).toBe(true);
    expect(next.nextCursor).toBe('page:older');
  });

  it('clears only the older segment when returning to the recent projection', () => {
    const history = createWorkbenchSnapshot().timelineHistory;
    history.items = [message(1)];
    history.nextCursor = 'page:older';
    history.gapAfter = true;

    expect(returnToRecent(history)).toMatchObject({
      items: [],
      retainedBytes: 0,
      gapAfter: false,
      phase: 'idle',
      error: null,
    });
  });
});
