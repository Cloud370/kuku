import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import taskProjectionJson from '../../api/generated/fixtures/task_projection.json';
import type { TaskProjection } from '../../api/generated';
import { applyProjection, createWorkbenchSnapshot } from '../state';
import { RunLiveRegion } from './RunLiveRegion';
import { StateBoundary } from './StateBoundary';

function projection(state: TaskProjection['task']['state']): TaskProjection {
  const value = structuredClone(taskProjectionJson) as TaskProjection;
  value.task.state = state;
  value.active_run =
    state === 'draft'
      ? null
      : {
          completion: null,
          finished_at: null,
          run_id: 'run_000000000000000000000001',
          started_at: '2026-07-18T00:00:00Z',
          state,
        };
  return value;
}

afterEach(() => {
  cleanup();
});

describe('presentation resilience', () => {
  it('keeps the last confirmed projection visible while reconnecting', () => {
    const snapshot = applyProjection(createWorkbenchSnapshot(), projection('running'));
    snapshot.connection = 'reconnecting';

    render(
      <StateBoundary onRetry={vi.fn()} snapshot={snapshot}>
        <p>last confirmed answer</p>
      </StateBoundary>,
    );

    expect(screen.getByText('Reconnecting')).toBeVisible();
    expect(screen.getByText('last confirmed answer')).toBeVisible();
    expect(screen.queryByText('Failed')).toBeNull();
  });

  it('announces meaningful run transitions without announcing token patches', () => {
    const running = projection('running');
    const view = render(<RunLiveRegion projection={running} />);
    expect(screen.getByRole('status', { name: 'Run status' })).toHaveTextContent('Run started');

    const tokenPatch = structuredClone(running);
    tokenPatch.cursor += 1;
    view.rerender(<RunLiveRegion projection={tokenPatch} />);
    expect(screen.getByRole('status', { name: 'Run status' })).toHaveTextContent('Run started');
    expect(screen.getByRole('status', { name: 'Run status' })).not.toHaveTextContent('token');

    view.rerender(<RunLiveRegion projection={projection('needs_attention')} />);
    expect(screen.getByRole('status', { name: 'Run status' })).toHaveTextContent(
      'Run needs attention',
    );
  });
});
