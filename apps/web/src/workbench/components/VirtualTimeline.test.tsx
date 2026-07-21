import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { VirtualTimeline } from './VirtualTimeline';

interface Item {
  id: string;
  text: string;
}

const items = (count: number): Item[] =>
  Array.from({ length: count }, (_, index) => ({
    id: `item-${String(index)}`,
    text: `Row ${String(index)}`,
  }));

afterEach(() => {
  cleanup();
});

describe('VirtualTimeline', () => {
  it('mounts at most 120 stable timeline rows for a large Task', () => {
    render(
      <main aria-label="Chat">
        <VirtualTimeline
          gapAfter={false}
          getItemId={(item) => item.id}
          items={items(10_000)}
          maxMountedRows={120}
          onReturnToRecent={vi.fn()}
          renderItem={(item) => item.text}
        />
      </main>,
    );

    expect(document.querySelectorAll('[data-timeline-id]').length).toBeLessThanOrEqual(120);
  });

  it('renders an honest history gap and returns to recent messages', async () => {
    const user = userEvent.setup();
    const onReturnToRecent = vi.fn();
    render(
      <VirtualTimeline
        gapAfter
        gapAfterIndex={2}
        getItemId={(item) => item.id}
        items={items(5)}
        maxMountedRows={120}
        onReturnToRecent={onReturnToRecent}
        renderItem={(item) => item.text}
      />,
    );

    expect(screen.getByText('Earlier history is separated from recent messages.')).toBeVisible();
    const gap = screen.getByText('Earlier history is separated from recent messages.');
    const earlier = document.querySelector('[data-timeline-id="item-1"]');
    const recent = document.querySelector('[data-timeline-id="item-2"]');
    expect(earlier).not.toBeNull();
    expect(recent).not.toBeNull();
    if (earlier === null || recent === null) throw new Error('timeline rows should be mounted');
    expect(earlier.compareDocumentPosition(gap)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(gap.compareDocumentPosition(recent)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    await user.click(screen.getByRole('button', { name: 'Return to recent messages' }));
    expect(onReturnToRecent).toHaveBeenCalledTimes(1);
  });

  it('does not create a nested scroll surface', () => {
    render(
      <VirtualTimeline
        gapAfter={false}
        getItemId={(item) => item.id}
        items={items(3)}
        maxMountedRows={120}
        onReturnToRecent={vi.fn()}
        renderItem={(item) => item.text}
      />,
    );

    expect(screen.getByTestId('virtual-timeline')).not.toHaveClass('overflow-auto');
  });

  it('preserves the first rendered stable ID offset after chronological prepend', async () => {
    const originalRect = Object.getOwnPropertyDescriptor(
      HTMLElement.prototype,
      'getBoundingClientRect',
    );
    HTMLElement.prototype.getBoundingClientRect = function getBoundingClientRect() {
      const main = this.closest<HTMLElement>('main[aria-label="Chat"]');
      const translatedTop = Number(/translateY\((\d+)px\)/.exec(this.style.transform)?.[1] ?? 0);
      const top = translatedTop - (main?.scrollTop ?? 0);
      return {
        bottom: top + 80,
        height: 80,
        left: 0,
        right: 800,
        toJSON: () => ({}),
        top,
        width: 800,
        x: 0,
        y: top,
      };
    };
    const initial = [
      { id: 'item-3', text: 'Row 3' },
      { id: 'item-4', text: 'Row 4' },
    ];
    const main = document.createElement('main');
    main.setAttribute('aria-label', 'Chat');
    document.body.append(main);
    const view = render(
      <VirtualTimeline
        gapAfter={false}
        getItemId={(item) => item.id}
        items={initial}
        maxMountedRows={120}
        onReturnToRecent={vi.fn()}
        renderItem={(item) => item.text}
      />,
      { container: main },
    );
    main.scrollTop = 50;
    fireEvent.scroll(main);

    view.rerender(
      <VirtualTimeline
        gapAfter={false}
        getItemId={(item) => item.id}
        items={[{ id: 'item-1', text: 'Row 1' }, { id: 'item-2', text: 'Row 2' }, ...initial]}
        maxMountedRows={120}
        onReturnToRecent={vi.fn()}
        renderItem={(item) => item.text}
      />,
    );

    await waitFor(() => {
      expect(main.scrollTop).toBe(210);
    });
    if (originalRect !== undefined) {
      Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', originalRect);
    }
    main.remove();
  });

  it('uses the measured anchor position when a large variable-height prepend exceeds overscan', async () => {
    const originalRect = Object.getOwnPropertyDescriptor(
      HTMLElement.prototype,
      'getBoundingClientRect',
    );
    const prepended = Array.from({ length: 200 }, (_, index) => ({
      id: `new-${String(index)}`,
      text: `New ${String(index)}`,
    }));
    const heights = new Map(prepended.map(({ id }, index) => [id, 32 + (index % 7) * 13] as const));
    const measuredIds = new Set<string>();
    const main = document.createElement('main');
    main.setAttribute('aria-label', 'Chat');
    document.body.append(main);
    HTMLElement.prototype.getBoundingClientRect = function getBoundingClientRect() {
      const measurementId = this.dataset.timelineMeasurementId;
      if (measurementId?.startsWith('new-') === true) measuredIds.add(measurementId);
      const id = measurementId ?? this.dataset.timelineId ?? '';
      const height = heights.get(id) ?? 80;
      const translatedTop = Number(/translateY\((\d+)px\)/.exec(this.style.transform)?.[1] ?? 0);
      const top = translatedTop - main.scrollTop;
      return {
        bottom: top + height,
        height,
        left: 0,
        right: 800,
        toJSON: () => ({}),
        top,
        width: 800,
        x: 0,
        y: top,
      };
    };
    const initial = Array.from({ length: 10 }, (_, index) => ({
      id: `old-${String(index)}`,
      text: `Old ${String(index)}`,
    }));
    const view = render(
      <VirtualTimeline
        gapAfter={false}
        getItemId={(item) => item.id}
        items={initial}
        maxMountedRows={10}
        onReturnToRecent={vi.fn()}
        renderItem={(item) => item.text}
      />,
      { container: main },
    );
    main.scrollTop = 25;
    fireEvent.scroll(main);

    view.rerender(
      <VirtualTimeline
        gapAfter={false}
        getItemId={(item) => item.id}
        items={[...prepended, ...initial]}
        maxMountedRows={10}
        onReturnToRecent={vi.fn()}
        renderItem={(item) => item.text}
      />,
    );

    await waitFor(() => {
      const measuredHeight = [...heights.values()].reduce((total, height) => total + height, 0);
      expect(measuredIds.size).toBe(prepended.length);
      expect(main.scrollTop).toBe(25 + measuredHeight);
    });
    if (originalRect !== undefined) {
      Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', originalRect);
    }
    main.remove();
  });

  it('preserves a visible anchor through a variable-height prepend and concurrent live append', async () => {
    const originalRect = Object.getOwnPropertyDescriptor(
      HTMLElement.prototype,
      'getBoundingClientRect',
    );
    const prepended = Array.from({ length: 200 }, (_, index) => ({
      id: `history-${String(index)}`,
      text: `History ${String(index)}`,
    }));
    const initial = Array.from({ length: 20 }, (_, index) => ({
      id: `stable-${String(index)}`,
      text: `Stable ${String(index)}`,
    }));
    const heights = new Map(prepended.map(({ id }, index) => [id, 35 + (index % 9) * 11] as const));
    const getItemId = (item: Item) => item.id;
    const renderItem = (item: Item) => item.text;
    const measuredIds = new Set<string>();
    HTMLElement.prototype.getBoundingClientRect = function getBoundingClientRect() {
      if (this.matches('main[aria-label="Chat"]')) {
        return {
          bottom: 800,
          height: 800,
          left: 0,
          right: 800,
          toJSON: () => ({}),
          top: 0,
          width: 800,
          x: 0,
          y: 0,
        };
      }
      const main = this.closest<HTMLElement>('main[aria-label="Chat"]');
      const measurementId = this.dataset.timelineMeasurementId;
      if (measurementId !== undefined) measuredIds.add(measurementId);
      const id = measurementId ?? this.dataset.timelineId ?? '';
      const height = heights.get(id) ?? 80;
      const translatedTop = Number(/translateY\((\d+)px\)/.exec(this.style.transform)?.[1] ?? 0);
      const layoutOffset = Number(
        main?.querySelector<HTMLElement>('[data-timeline-layout-offset]')?.dataset
          .timelineLayoutOffset ?? 0,
      );
      const top = layoutOffset + translatedTop - (main?.scrollTop ?? 0);
      return {
        bottom: top + height,
        height,
        left: 0,
        right: 800,
        toJSON: () => ({}),
        top,
        width: 800,
        x: 0,
        y: top,
      };
    };
    const fixture = (timelineItems: Item[], layoutOffset: number) => (
      <main aria-label="Chat">
        <div data-timeline-layout-offset={layoutOffset} />
        <VirtualTimeline
          gapAfter={false}
          getItemId={getItemId}
          items={timelineItems}
          maxMountedRows={10}
          onReturnToRecent={vi.fn()}
          renderItem={renderItem}
        />
      </main>
    );
    const view = render(fixture(initial, 200));
    const main = screen.getByRole('main', { name: 'Chat' });
    main.scrollTop = 200;
    fireEvent.scroll(main);
    const anchor = document.querySelector<HTMLElement>('[data-timeline-id="stable-0"]');
    if (anchor === null) throw new Error('the initial visible anchor should be mounted');
    const initialTop = anchor.getBoundingClientRect().top;

    view.rerender(
      fixture(
        [...prepended, ...initial, { id: 'live-append', text: 'Concurrent live append' }],
        100,
      ),
    );

    await waitFor(() => {
      expect(measuredIds.size).toBe(prepended.length);
      const preserved = document.querySelector<HTMLElement>('[data-timeline-id="stable-0"]');
      if (preserved === null) throw new Error('the stable anchor should remain mounted');
      expect(preserved.getBoundingClientRect().top).toBeCloseTo(initialTop, 5);
      expect(document.querySelectorAll('[data-timeline-id]').length).toBeLessThanOrEqual(10);
    });

    main.scrollTop = 0;
    fireEvent.scroll(main);
    fireEvent.scroll(main);
    const visibleAfterScroll = await waitFor(() => {
      const candidate = Array.from(document.querySelectorAll<HTMLElement>('[data-timeline-id]'))
        .filter((row) => {
          const rect = row.getBoundingClientRect();
          return rect.top >= 0 && rect.bottom <= 800;
        })
        .sort(
          (left, right) => left.getBoundingClientRect().top - right.getBoundingClientRect().top,
        )[0];
      expect(candidate).toBeDefined();
      expect(candidate?.dataset.timelineId).not.toBe('stable-0');
      return candidate as HTMLElement;
    });
    const secondAnchorId = visibleAfterScroll.dataset.timelineId;
    if (secondAnchorId === undefined)
      throw new Error('the scrolled anchor should expose a stable ID');
    const secondAnchorTop = visibleAfterScroll.getBoundingClientRect().top;
    const older = Array.from({ length: 30 }, (_, index) => ({
      id: `older-${String(index)}`,
      text: `Older ${String(index)}`,
    }));
    for (const [index, item] of older.entries()) heights.set(item.id, 41 + (index % 5) * 17);

    view.rerender(
      fixture(
        [...older, ...prepended, ...initial, { id: 'live-append', text: 'Concurrent live append' }],
        100,
      ),
    );

    await waitFor(() => {
      expect(measuredIds.size).toBe(prepended.length + older.length);
      const preserved = document.querySelector<HTMLElement>(
        `[data-timeline-id=${JSON.stringify(secondAnchorId)}]`,
      );
      if (preserved === null) throw new Error('the scrolled anchor should survive another prepend');
      expect(preserved.getBoundingClientRect().top).toBeCloseTo(secondAnchorTop, 5);
      expect(document.querySelectorAll('[data-timeline-id]').length).toBeLessThanOrEqual(10);
    });
    if (originalRect !== undefined) {
      Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', originalRect);
    }
  });

  it('accounts for retained-history eviction and releases the anchor after a same-row scroll', async () => {
    const originalRect = Object.getOwnPropertyDescriptor(
      HTMLElement.prototype,
      'getBoundingClientRect',
    );
    const originalResizeObserver = window.ResizeObserver;
    const resizeObservers: Array<{
      callback: ResizeObserverCallback;
      observed: Set<Element>;
      observer: ResizeObserver;
    }> = [];
    class TestResizeObserver implements ResizeObserver {
      private readonly record: (typeof resizeObservers)[number];

      constructor(callback: ResizeObserverCallback) {
        this.record = { callback, observed: new Set(), observer: this };
        resizeObservers.push(this.record);
      }

      disconnect() {
        this.record.observed.clear();
      }

      observe(target: Element) {
        this.record.observed.add(target);
      }

      unobserve(target: Element) {
        this.record.observed.delete(target);
      }
    }
    window.ResizeObserver = TestResizeObserver;
    const heights = new Map<string, number>();
    const evicted = Array.from({ length: 5 }, (_, index) => ({
      id: `evicted-${String(index)}`,
      text: `Evicted ${String(index)}`,
    }));
    for (const item of evicted) heights.set(item.id, 80);
    const anchor = { id: 'retained-anchor', text: 'Retained anchor' };
    const tail = Array.from({ length: 3 }, (_, index) => ({
      id: `tail-${String(index)}`,
      text: `Tail ${String(index)}`,
    }));
    const prepended = Array.from({ length: 20 }, (_, index) => ({
      id: `retained-history-${String(index)}`,
      text: `Retained history ${String(index)}`,
    }));
    for (const [index, item] of prepended.entries()) heights.set(item.id, 37 + (index % 4) * 19);
    const measuredIds = new Set<string>();
    HTMLElement.prototype.getBoundingClientRect = function getBoundingClientRect() {
      if (this.matches('main[aria-label="Chat"]')) {
        return {
          bottom: 800,
          height: 800,
          left: 0,
          right: 800,
          toJSON: () => ({}),
          top: 0,
          width: 800,
          x: 0,
          y: 0,
        };
      }
      const main = this.closest<HTMLElement>('main[aria-label="Chat"]');
      const measurementId = this.dataset.timelineMeasurementId;
      if (measurementId !== undefined) measuredIds.add(measurementId);
      const id = measurementId ?? this.dataset.timelineId ?? '';
      const height = heights.get(id) ?? 80;
      const translatedTop = Number(/translateY\((\d+)px\)/.exec(this.style.transform)?.[1] ?? 0);
      const top = translatedTop - (main?.scrollTop ?? 0);
      return {
        bottom: top + height,
        height,
        left: 0,
        right: 800,
        toJSON: () => ({}),
        top,
        width: 800,
        x: 0,
        y: top,
      };
    };
    const fixture = (timelineItems: Item[], gapAfter: boolean, gapAfterIndex?: number) => (
      <main aria-label="Chat">
        <VirtualTimeline
          gapAfter={gapAfter}
          gapAfterIndex={gapAfterIndex}
          getItemId={(item) => item.id}
          items={timelineItems}
          maxMountedRows={30}
          onReturnToRecent={vi.fn()}
          renderItem={(item) => item.text}
        />
      </main>
    );
    const view = render(fixture([...evicted, anchor, ...tail], true, evicted.length));
    const main = screen.getByRole('main', { name: 'Chat' });
    const initialAnchor = document.querySelector<HTMLElement>(
      '[data-timeline-id="retained-anchor"]',
    );
    if (initialAnchor === null) throw new Error('the retained anchor should be mounted');
    main.scrollTop = Number(/translateY\((\d+)px\)/.exec(initialAnchor.style.transform)?.[1] ?? 0);
    fireEvent.scroll(main);
    fireEvent.scroll(main);
    const initialTop = initialAnchor.getBoundingClientRect().top;

    view.rerender(fixture([...prepended, anchor, ...tail], true, prepended.length));

    const prependedHeight = [...prepended].reduce(
      (total, item) => total + (heights.get(item.id) ?? 0),
      0,
    );
    const expectedStart = prependedHeight + 96;
    await waitFor(() => {
      expect(measuredIds.size).toBe(prepended.length);
      const preserved = document.querySelector<HTMLElement>('[data-timeline-id="retained-anchor"]');
      if (preserved === null) throw new Error('retention should preserve the anchor');
      expect(preserved.style.transform).toBe(`translateY(${String(expectedStart)}px)`);
      expect(preserved.getBoundingClientRect().top).toBeCloseTo(initialTop, 5);
    });

    main.scrollTop += 10;
    fireEvent.scroll(main);
    const scrolledTop = document
      .querySelector<HTMLElement>('[data-timeline-id="retained-anchor"]')
      ?.getBoundingClientRect().top;
    if (scrolledTop === undefined) throw new Error('the same anchor should remain visible');
    const remeasured = document.querySelector<HTMLElement>(
      '[data-timeline-id="retained-history-19"]',
    );
    if (remeasured === null) throw new Error('an above-anchor row should remain mounted');
    const remeasuredHeight = (heights.get('retained-history-19') ?? 0) + 43;
    heights.set('retained-history-19', remeasuredHeight);
    act(() => {
      for (const resizeObserver of resizeObservers) {
        if (!resizeObserver.observed.has(remeasured)) continue;
        resizeObserver.callback(
          [
            {
              borderBoxSize: [{ blockSize: remeasuredHeight, inlineSize: 800 }],
              target: remeasured,
            } as unknown as ResizeObserverEntry,
          ],
          resizeObserver.observer,
        );
      }
    });

    await waitFor(() => {
      const preserved = document.querySelector<HTMLElement>('[data-timeline-id="retained-anchor"]');
      if (preserved === null) throw new Error('remeasurement should retain the same anchor');
      expect(preserved.style.transform).toBe(`translateY(${String(expectedStart + 43)}px)`);
      expect(preserved.getBoundingClientRect().top).toBeCloseTo(scrolledTop, 5);
    });
    const removed = prepended[0];
    if (removed === undefined) throw new Error('the retention fixture should have a prefix');
    view.rerender(fixture([...prepended.slice(1), anchor, ...tail], true, prepended.length - 1));

    await waitFor(() => {
      const preserved = document.querySelector<HTMLElement>('[data-timeline-id="retained-anchor"]');
      if (preserved === null) throw new Error('the same-row scroll should retain its anchor');
      expect(preserved.style.transform).toBe(
        `translateY(${String(expectedStart + 43 - (heights.get(removed.id) ?? 0))}px)`,
      );
      expect(preserved.getBoundingClientRect().top).toBeCloseTo(scrolledTop, 5);
    });
    if (originalRect !== undefined) {
      Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', originalRect);
    }
    window.ResizeObserver = originalResizeObserver;
  });
});
