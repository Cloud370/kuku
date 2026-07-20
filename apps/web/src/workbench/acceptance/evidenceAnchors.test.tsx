import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import taskProjectionJson from '../../api/generated/fixtures/task_projection.json';
import type { TaskProjection } from '../../api/generated';
import { applyProjection, createWorkbenchSnapshot } from '../state';
import { RunLiveRegion } from '../components/RunLiveRegion';
import { StateBoundary } from '../components/StateBoundary';
import { ChatTimeline } from '../components/ChatTimeline';

afterEach(() => {
  cleanup();
});

describe('H1 evidence anchors', () => {
  it('exposes a silent Conversation region and a separate named run status', () => {
    const projection = structuredClone(taskProjectionJson) as TaskProjection;
    const snapshot = applyProjection(createWorkbenchSnapshot(), projection);
    render(
      <StateBoundary onRetry={vi.fn()} snapshot={snapshot}>
        <ChatTimeline
          loadOlder={vi.fn()}
          onOpenFile={vi.fn()}
          onOpenRequestContext={vi.fn()}
          onOpenReview={vi.fn()}
          onRespond={vi.fn()}
          onReturnToRecent={vi.fn()}
          projection={projection}
          timelineHistory={snapshot.timelineHistory}
          timelineItems={projection.timeline}
        />
        <RunLiveRegion projection={projection} />
      </StateBoundary>,
    );

    expect(screen.getByRole('region', { name: 'Conversation' })).not.toHaveAttribute('aria-live');
    expect(screen.getByRole('status', { name: 'Run status' })).toBeInTheDocument();
  });
});
