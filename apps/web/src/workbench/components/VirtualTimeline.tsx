import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual';
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';

interface VirtualTimelineProps<Item> {
  gapAfter: boolean;
  gapAfterIndex?: number;
  getItemId: (item: Item) => string;
  items: Item[];
  maxMountedRows: number;
  onReturnToRecent: () => void;
  renderItem: (item: Item) => ReactNode;
}

interface Anchor {
  id: string;
  top: number;
}

interface PendingPrepend {
  height: number;
  scrollTop: number;
}

export function VirtualTimeline<Item>({
  gapAfter,
  gapAfterIndex,
  getItemId,
  items,
  maxMountedRows,
  onReturnToRecent,
  renderItem,
}: VirtualTimelineProps<Item>) {
  const rootRef = useRef<HTMLDivElement>(null);
  const anchorSnapshotRef = useRef<Anchor | null>(null);
  const measuredPrependIdsRef = useRef(new Set<string>());
  const pendingPrependRef = useRef<PendingPrepend | null>(null);
  const previousFirstIdRef = useRef(items[0] === undefined ? null : getItemId(items[0]));
  const [measurementRevision, setMeasurementRevision] = useState(0);
  const requestedGapIndex = gapAfterIndex ?? items.length;
  const gapIndex = gapAfter ? Math.max(0, Math.min(requestedGapIndex, items.length)) : null;
  const virtualCount = items.length + (gapIndex === null ? 0 : 1);
  const itemIndex = (virtualIndex: number): number | null => {
    if (gapIndex === null) return virtualIndex;
    if (virtualIndex === gapIndex) return null;
    return virtualIndex > gapIndex ? virtualIndex - 1 : virtualIndex;
  };
  const getAnchorVirtualIndex = (): number | null => {
    const anchor = anchorSnapshotRef.current;
    if (anchor === null) return null;
    const currentIndex = items.findIndex((item) => getItemId(item) === anchor.id);
    if (currentIndex < 0) return null;
    return gapIndex !== null && currentIndex >= gapIndex ? currentIndex + 1 : currentIndex;
  };
  const virtualizer = useVirtualizer({
    count: virtualCount,
    estimateSize: () => 96,
    getItemKey: (index) => {
      const resolvedIndex = itemIndex(index);
      return resolvedIndex === null
        ? 'timeline-history-gap'
        : getItemId(items[resolvedIndex] as Item);
    },
    getScrollElement: () =>
      rootRef.current?.closest<HTMLElement>('main[aria-label="Chat"]') ?? null,
    initialRect: { height: 800, width: 800 },
    overscan: 8,
    rangeExtractor: (range) => {
      const indexes = defaultRangeExtractor(range);
      const anchorVirtualIndex = getAnchorVirtualIndex();
      return anchorVirtualIndex === null || indexes.includes(anchorVirtualIndex)
        ? indexes
        : [...indexes, anchorVirtualIndex].sort((left, right) => left - right);
    },
  });
  const allMeasuredRows = virtualizer.getVirtualItems();
  const anchorVirtualIndex = getAnchorVirtualIndex();
  const measuredRows = allMeasuredRows.slice(0, maxMountedRows);
  if (
    anchorVirtualIndex !== null &&
    maxMountedRows > 0 &&
    !measuredRows.some((row) => row.index === anchorVirtualIndex)
  ) {
    const anchorRow = allMeasuredRows.find((row) => row.index === anchorVirtualIndex);
    if (anchorRow !== undefined) {
      measuredRows.splice(maxMountedRows - 1, 1, anchorRow);
      measuredRows.sort((left, right) => left.index - right.index);
    }
  }
  const fallbackIndexes = Array.from(
    { length: Math.min(virtualCount, maxMountedRows) },
    (_, index) => index,
  );
  if (
    anchorVirtualIndex !== null &&
    fallbackIndexes.length > 0 &&
    !fallbackIndexes.includes(anchorVirtualIndex)
  ) {
    fallbackIndexes[fallbackIndexes.length - 1] = anchorVirtualIndex;
    fallbackIndexes.sort((left, right) => left - right);
  }
  const previousFirstId = previousFirstIdRef.current;
  const prependCount =
    previousFirstId === null ? 0 : items.findIndex((item) => getItemId(item) === previousFirstId);
  const measurementItems = items
    .slice(0, Math.max(0, prependCount))
    .map((item, index) => ({
      id: getItemId(item),
      item,
      virtualIndex: gapIndex !== null && index >= gapIndex ? index + 1 : index,
    }))
    .filter(({ id }) => !measuredPrependIdsRef.current.has(id))
    .slice(0, Math.max(1, maxMountedRows));
  const virtualRows =
    measuredRows.length > 0
      ? measuredRows
      : fallbackIndexes.map((index) => ({
          end: (index + 1) * 96,
          index,
          key: index,
          size: 96,
          start: index * 96,
        }));

  const captureAnchor = () => {
    const rows = Array.from(
      rootRef.current?.querySelectorAll<HTMLElement>('[data-timeline-id]') ?? [],
    );
    const currentAnchor = anchorSnapshotRef.current;
    const row =
      rows.find((candidate) => candidate.dataset.timelineId === currentAnchor?.id) ?? rows[0];
    const id = row?.dataset.timelineId;
    anchorSnapshotRef.current =
      row === undefined || id === undefined
        ? null
        : {
            id,
            top: row.getBoundingClientRect().top,
          };
  };

  useLayoutEffect(
    () => () => {
      captureAnchor();
    },
    [getItemId, items],
  );

  useLayoutEffect(() => {
    const scrollElement = rootRef.current?.closest<HTMLElement>('main[aria-label="Chat"]') ?? null;
    const measurementRows = Array.from(
      rootRef.current?.querySelectorAll<HTMLElement>('[data-timeline-measurement-id]') ?? [],
    );
    if (measurementRows.length > 0) {
      if (pendingPrependRef.current === null && scrollElement !== null) {
        pendingPrependRef.current = { height: 0, scrollTop: scrollElement.scrollTop };
      }
      let measuredAny = false;
      for (const row of measurementRows) {
        const id = row.dataset.timelineMeasurementId;
        const index = Number(row.dataset.index);
        const height = row.getBoundingClientRect().height;
        if (id !== undefined && Number.isInteger(index) && height > 0) {
          virtualizer.resizeItem(index, height);
          measuredPrependIdsRef.current.add(id);
          if (pendingPrependRef.current !== null) {
            pendingPrependRef.current.height += height;
          }
          measuredAny = true;
        }
      }
      if (measuredAny) {
        setMeasurementRevision((current) => current + 1);
        return;
      }
    }
    const pendingPrepend = pendingPrependRef.current;
    if (pendingPrepend !== null && scrollElement !== null) {
      scrollElement.scrollTop = pendingPrepend.scrollTop + pendingPrepend.height;
      pendingPrependRef.current = null;
      measuredPrependIdsRef.current.clear();
      previousFirstIdRef.current = items[0] === undefined ? null : getItemId(items[0]);
      captureAnchor();
      return;
    }
    const rows = Array.from(
      rootRef.current?.querySelectorAll<HTMLElement>('[data-timeline-id]') ?? [],
    );
    const previousAnchor = anchorSnapshotRef.current;
    if (previousAnchor !== null && scrollElement !== null) {
      const anchoredRow = rows.find((row) => row.dataset.timelineId === previousAnchor.id);
      if (anchoredRow !== undefined) {
        scrollElement.scrollTop += anchoredRow.getBoundingClientRect().top - previousAnchor.top;
        anchorSnapshotRef.current = {
          id: previousAnchor.id,
          top: anchoredRow.getBoundingClientRect().top,
        };
        return;
      }
    }
    previousFirstIdRef.current = items[0] === undefined ? null : getItemId(items[0]);
    captureAnchor();
  }, [getItemId, items, measurementRevision, virtualizer]);

  return (
    <div className="relative min-w-0" data-testid="virtual-timeline" ref={rootRef}>
      {measurementItems.length > 0 ? (
        <div
          aria-hidden="true"
          className="pointer-events-none invisible absolute left-0 top-0 w-full"
        >
          {measurementItems.map(({ id, item, virtualIndex }) => (
            <div
              className="w-full px-4 py-3"
              data-index={virtualIndex}
              data-timeline-measurement-id={id}
              key={`measurement:${id}`}
            >
              {renderItem(item)}
            </div>
          ))}
        </div>
      ) : null}
      <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
        {virtualRows.map((virtualRow) => {
          const resolvedIndex = itemIndex(virtualRow.index);
          if (resolvedIndex === null) {
            return (
              <div
                className="absolute left-0 top-0 w-full px-4 py-3"
                data-index={virtualRow.index}
                key="timeline-history-gap"
                ref={virtualizer.measureElement}
                style={{ transform: `translateY(${String(virtualRow.start)}px)` }}
              >
                <div className="border-t border-dashed border-[var(--color-border)] py-4 text-center">
                  <p className="text-xs text-[var(--color-text-secondary)]">
                    Earlier history is separated from recent messages.
                  </p>
                  <button
                    className="mt-2 text-xs font-medium text-[var(--color-accent)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
                    onClick={onReturnToRecent}
                    type="button"
                  >
                    Return to recent messages
                  </button>
                </div>
              </div>
            );
          }
          const item = items[resolvedIndex] as Item;
          const id = getItemId(item);
          return (
            <div
              className="absolute left-0 top-0 w-full px-4 py-3"
              data-index={virtualRow.index}
              data-timeline-id={id}
              key={id}
              ref={virtualizer.measureElement}
              style={{ transform: `translateY(${String(virtualRow.start)}px)` }}
            >
              {renderItem(item)}
            </div>
          );
        })}
      </div>
    </div>
  );
}
