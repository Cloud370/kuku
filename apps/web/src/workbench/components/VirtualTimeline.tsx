import { useVirtualizer } from '@tanstack/react-virtual';
import { useLayoutEffect, useRef, type ReactNode } from 'react';

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
  index: number;
  top: number;
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
  const requestedGapIndex = gapAfterIndex ?? items.length;
  const gapIndex = gapAfter ? Math.max(0, Math.min(requestedGapIndex, items.length)) : null;
  const virtualCount = items.length + (gapIndex === null ? 0 : 1);
  const itemIndex = (virtualIndex: number): number | null => {
    if (gapIndex === null) return virtualIndex;
    if (virtualIndex === gapIndex) return null;
    return virtualIndex > gapIndex ? virtualIndex - 1 : virtualIndex;
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
  });
  const measuredRows = virtualizer.getVirtualItems().slice(0, maxMountedRows);
  const virtualRows =
    measuredRows.length > 0
      ? measuredRows
      : Array.from({ length: Math.min(virtualCount, maxMountedRows) }, (_, index) => ({
          end: (index + 1) * 96,
          index,
          key: index,
          size: 96,
          start: index * 96,
        }));

  useLayoutEffect(() => {
    const scrollElement = rootRef.current?.closest<HTMLElement>('main[aria-label="Chat"]') ?? null;
    const rows = Array.from(
      rootRef.current?.querySelectorAll<HTMLElement>('[data-timeline-id]') ?? [],
    );
    const previousAnchor = anchorSnapshotRef.current;
    if (previousAnchor !== null && scrollElement !== null) {
      const anchoredRow = rows.find((row) => row.dataset.timelineId === previousAnchor.id);
      if (anchoredRow !== undefined) {
        scrollElement.scrollTop += anchoredRow.getBoundingClientRect().top - previousAnchor.top;
      } else {
        const currentIndex = items.findIndex((item) => getItemId(item) === previousAnchor.id);
        if (currentIndex >= 0) {
          scrollElement.scrollTop += (currentIndex - previousAnchor.index) * 96;
        }
      }
    }
    const firstRow = rows[0];
    const id = firstRow?.dataset.timelineId;
    anchorSnapshotRef.current =
      firstRow === undefined || id === undefined
        ? null
        : {
            id,
            index: items.findIndex((item) => getItemId(item) === id),
            top: firstRow.getBoundingClientRect().top,
          };
  }, [getItemId, items]);

  return (
    <div className="relative min-w-0" data-testid="virtual-timeline" ref={rootRef}>
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
