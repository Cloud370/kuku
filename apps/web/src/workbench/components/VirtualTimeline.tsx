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
  start: number;
  top: number;
}

interface AnchoredPosition {
  id: string;
  start: number;
}

interface PendingPrepend {
  scrollTop: number;
}

interface TimelineLayout {
  signature: string;
  size: number;
  starts: number[];
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
  const anchoredPositionRef = useRef<AnchoredPosition | null>(null);
  const measuredPrependIdsRef = useRef(new Set<string>());
  const pendingPrependRef = useRef<PendingPrepend | null>(null);
  const previousItemIdsRef = useRef(new Set(items.map((item) => getItemId(item))));
  const layoutRef = useRef<TimelineLayout | null>(null);
  const anchorVirtualIndexRef = useRef<number | null>(null);
  const layoutStartsRef = useRef<number[]>([]);
  const captureScrolledAnchorRef = useRef<() => Anchor | null>(() => null);
  const [measurementRevision, setMeasurementRevision] = useState(0);
  const requestedGapIndex = gapAfterIndex ?? items.length;
  const gapIndex = gapAfter ? Math.max(0, Math.min(requestedGapIndex, items.length)) : null;
  const virtualCount = items.length + (gapIndex === null ? 0 : 1);
  const itemIndex = (virtualIndex: number): number | null => {
    if (gapIndex === null) return virtualIndex;
    if (virtualIndex === gapIndex) return null;
    return virtualIndex > gapIndex ? virtualIndex - 1 : virtualIndex;
  };
  const itemKey = (virtualIndex: number): string => {
    const resolvedIndex = itemIndex(virtualIndex);
    return resolvedIndex === null
      ? 'timeline-history-gap'
      : getItemId(items[resolvedIndex] as Item);
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
    getItemKey: itemKey,
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
  const itemSize = (index: number): number => {
    const measuredSize = virtualizer.itemSizeCache.get(itemKey(index));
    return measuredSize !== undefined && measuredSize > 0 ? measuredSize : 96;
  };
  const layoutSizes = Array.from({ length: virtualCount }, (_, index) => itemSize(index));
  const layoutSignature = layoutSizes.join(',');
  if (layoutRef.current === null || layoutRef.current.signature !== layoutSignature) {
    const starts: number[] = [];
    let size = 0;
    for (const rowSize of layoutSizes) {
      starts.push(size);
      size += rowSize;
    }
    layoutRef.current = { signature: layoutSignature, size, starts };
  }
  const { size: layoutSize, starts: layoutStarts } = layoutRef.current;
  anchorVirtualIndexRef.current = anchorVirtualIndex;
  layoutStartsRef.current = layoutStarts;
  const measuredRows =
    virtualCount <= maxMountedRows
      ? Array.from({ length: virtualCount }, (_, index) => {
          const start = layoutStarts[index] ?? 0;
          const size = itemSize(index);
          return {
            end: start + size,
            index,
            key: itemKey(index),
            lane: 0,
            size,
            start,
          };
        })
      : allMeasuredRows.slice(0, maxMountedRows);
  if (
    anchorVirtualIndex !== null &&
    maxMountedRows > 0 &&
    !measuredRows.some((row) => row.index === anchorVirtualIndex)
  ) {
    const anchorStart = layoutStarts[anchorVirtualIndex] ?? 0;
    const anchorSize = itemSize(anchorVirtualIndex);
    const anchorRow = allMeasuredRows.find((row) => row.index === anchorVirtualIndex) ?? {
      end: anchorStart + anchorSize,
      index: anchorVirtualIndex,
      key: itemKey(anchorVirtualIndex),
      lane: 0,
      size: anchorSize,
      start: anchorStart,
    };
    if (measuredRows.length >= maxMountedRows) {
      measuredRows.splice(maxMountedRows - 1, 1, anchorRow);
    } else {
      measuredRows.push(anchorRow);
    }
    measuredRows.sort((left, right) => left.index - right.index);
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
  const firstRetainedIndex = items.findIndex((item) =>
    previousItemIdsRef.current.has(getItemId(item)),
  );
  const prependCount = firstRetainedIndex < 0 ? 0 : firstRetainedIndex;
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
  const captureAnchor = (preferCurrent = true): Anchor | null => {
    const rows = Array.from(
      rootRef.current?.querySelectorAll<HTMLElement>('[data-timeline-id]') ?? [],
    );
    const currentAnchor = anchorSnapshotRef.current;
    const scrollElement = rootRef.current?.closest<HTMLElement>('main[aria-label="Chat"]') ?? null;
    const viewport = scrollElement?.getBoundingClientRect();
    const visibleRow =
      viewport === undefined
        ? undefined
        : rows
            .filter((candidate) => {
              const rect = candidate.getBoundingClientRect();
              return rect.bottom > viewport.top && rect.top < viewport.bottom;
            })
            .sort(
              (left, right) => left.getBoundingClientRect().top - right.getBoundingClientRect().top,
            )[0];
    const row = preferCurrent
      ? (rows.find((candidate) => candidate.dataset.timelineId === currentAnchor?.id) ??
        visibleRow ??
        rows[0])
      : (visibleRow ?? rows[0]);
    const id = row?.dataset.timelineId;
    const nextAnchor =
      row === undefined || id === undefined
        ? null
        : {
            id,
            start: timelineRowStart(row),
            top: row.getBoundingClientRect().top,
          };
    if (anchoredPositionRef.current !== null && anchoredPositionRef.current.id !== nextAnchor?.id) {
      anchoredPositionRef.current = null;
    }
    anchorSnapshotRef.current = nextAnchor;
    return nextAnchor;
  };

  const captureLayoutAnchor = (): Anchor | null => {
    const scrollElement = rootRef.current?.closest<HTMLElement>('main[aria-label="Chat"]') ?? null;
    const root = rootRef.current;
    if (scrollElement === null || root === null) return null;
    const viewport = scrollElement.getBoundingClientRect();
    const rootTop = root.getBoundingClientRect().top;
    const viewportTop = viewport.top - rootTop;
    const viewportBottom = viewport.bottom - rootTop;
    for (let index = 0; index < virtualCount; index += 1) {
      const resolvedIndex = itemIndex(index);
      const start = layoutStarts[index] ?? 0;
      const end = index + 1 < virtualCount ? (layoutStarts[index + 1] ?? start) : layoutSize;
      if (resolvedIndex !== null && end > viewportTop && start < viewportBottom) {
        const nextAnchor = {
          id: getItemId(items[resolvedIndex] as Item),
          start,
          top: rootTop + start,
        };
        anchorSnapshotRef.current = nextAnchor;
        return nextAnchor;
      }
    }
    return null;
  };
  captureScrolledAnchorRef.current = () => captureLayoutAnchor() ?? captureAnchor(false);

  useLayoutEffect(() => {
    const scrollElement = rootRef.current?.closest<HTMLElement>('main[aria-label="Chat"]') ?? null;
    if (scrollElement === null) return;
    const captureScrolledAnchor = () => {
      if (pendingPrependRef.current !== null) return;
      const hadAnchoredPosition = anchoredPositionRef.current !== null;
      anchoredPositionRef.current = null;
      captureScrolledAnchorRef.current();
      if (hadAnchoredPosition) setMeasurementRevision((current) => current + 1);
    };
    scrollElement.addEventListener('scroll', captureScrolledAnchor, { passive: true });
    return () => {
      scrollElement.removeEventListener('scroll', captureScrolledAnchor);
    };
  }, [getItemId, items, measurementRevision]);

  useLayoutEffect(() => {
    const scrollElement = rootRef.current?.closest<HTMLElement>('main[aria-label="Chat"]') ?? null;
    const measurementRows = Array.from(
      rootRef.current?.querySelectorAll<HTMLElement>('[data-timeline-measurement-id]') ?? [],
    );
    if (measurementRows.length > 0) {
      if (pendingPrependRef.current === null && scrollElement !== null) {
        pendingPrependRef.current = {
          scrollTop: scrollElement.scrollTop,
        };
      }
      let measuredAny = false;
      for (const row of measurementRows) {
        const id = row.dataset.timelineMeasurementId;
        const index = Number(row.dataset.index);
        const height = row.getBoundingClientRect().height;
        if (id !== undefined && Number.isInteger(index) && height > 0) {
          virtualizer.resizeItem(index, height);
          measuredPrependIdsRef.current.add(id);
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
      const previousAnchor = anchorSnapshotRef.current;
      const currentAnchorVirtualIndex = anchorVirtualIndexRef.current;
      const anchorStart =
        currentAnchorVirtualIndex === null
          ? (previousAnchor?.start ?? 0)
          : (layoutStartsRef.current[currentAnchorVirtualIndex] ?? 0);
      if (previousAnchor !== null) {
        anchoredPositionRef.current = {
          id: previousAnchor.id,
          start: anchorStart,
        };
      }
      scrollElement.scrollTop =
        pendingPrepend.scrollTop + anchorStart - (previousAnchor?.start ?? anchorStart);
      const anchoredRow = Array.from(
        rootRef.current?.querySelectorAll<HTMLElement>('[data-timeline-id]') ?? [],
      ).find((row) => row.dataset.timelineId === previousAnchor?.id);
      if (anchoredRow !== undefined && previousAnchor !== null) {
        const anchoredPosition = anchoredPositionRef.current;
        if (anchoredPosition !== null) {
          anchoredRow.style.transform = `translateY(${String(anchoredPosition.start)}px)`;
        }
        scrollElement.scrollTop += anchoredRow.getBoundingClientRect().top - previousAnchor.top;
      }
      pendingPrependRef.current = null;
      measuredPrependIdsRef.current.clear();
      previousItemIdsRef.current = new Set(items.map((item) => getItemId(item)));
      captureAnchor();
      setMeasurementRevision((current) => current + 1);
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
          start: timelineRowStart(anchoredRow),
          top: anchoredRow.getBoundingClientRect().top,
        };
        previousItemIdsRef.current = new Set(items.map((item) => getItemId(item)));
        return;
      }
    }
    previousItemIdsRef.current = new Set(items.map((item) => getItemId(item)));
    captureAnchor();
  }, [getItemId, items, layoutSize, measurementRevision, virtualizer]);

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
      <div className="relative w-full" style={{ height: layoutSize }}>
        {virtualRows.map((virtualRow) => {
          const resolvedIndex = itemIndex(virtualRow.index);
          if (resolvedIndex === null) {
            return (
              <div
                className="absolute left-0 top-0 w-full px-4 py-3"
                data-index={virtualRow.index}
                key="timeline-history-gap"
                ref={virtualizer.measureElement}
                style={{
                  transform: `translateY(${String(layoutStarts[virtualRow.index] ?? virtualRow.start)}px)`,
                }}
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
          const anchoredPosition = anchoredPositionRef.current;
          const start =
            anchoredPosition?.id === id
              ? anchoredPosition.start
              : (layoutStarts[virtualRow.index] ?? virtualRow.start);
          return (
            <div
              className="absolute left-0 top-0 w-full px-4 py-3"
              data-index={virtualRow.index}
              data-timeline-id={id}
              key={id}
              ref={virtualizer.measureElement}
              style={{ transform: `translateY(${String(start)}px)` }}
            >
              {renderItem(item)}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function timelineRowStart(row: HTMLElement): number {
  return Number(/translateY\((-?\d+(?:\.\d+)?)px\)/.exec(row.style.transform)?.[1] ?? 0);
}
