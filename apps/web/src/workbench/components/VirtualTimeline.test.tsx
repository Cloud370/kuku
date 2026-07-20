import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
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
      const index = Number(this.dataset.index ?? 0);
      return {
        bottom: index * 100 + 80,
        height: 80,
        left: 0,
        right: 800,
        toJSON: () => ({}),
        top: index * 100,
        width: 800,
        x: 0,
        y: index * 100,
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
      expect(main.scrollTop).toBe(250);
    });
    if (originalRect !== undefined) {
      Object.defineProperty(HTMLElement.prototype, 'getBoundingClientRect', originalRect);
    }
    main.remove();
  });

  it('uses the row estimate when a large prepend moves the anchor beyond overscan', async () => {
    const initial = Array.from({ length: 10 }, (_, index) => ({
      id: `old-${String(index)}`,
      text: `Old ${String(index)}`,
    }));
    const main = document.createElement('main');
    main.setAttribute('aria-label', 'Chat');
    document.body.append(main);
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

    view.rerender(
      <VirtualTimeline
        gapAfter={false}
        getItemId={(item) => item.id}
        items={[
          ...Array.from({ length: 200 }, (_, index) => ({
            id: `new-${String(index)}`,
            text: `New ${String(index)}`,
          })),
          ...initial,
        ]}
        maxMountedRows={10}
        onReturnToRecent={vi.fn()}
        renderItem={(item) => item.text}
      />,
    );

    await waitFor(() => {
      expect(main.scrollTop).toBe(25 + 200 * 96);
    });
    main.remove();
  });
});
