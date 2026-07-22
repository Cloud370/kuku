import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { MetricProjection, RunProjection } from '../../api/generated';

import { RunStatusBanner } from './RunStatusBanner';

function completedRun(metrics: MetricProjection[] | null): RunProjection {
  return {
    completion: {
      checks: null,
      metrics,
      summary: 'Finished',
      warnings: [],
      workspace_changes: null,
    },
    finished_at: '2026-07-22T01:00:00Z',
    run_id: 'run_000000000000000000000001',
    started_at: '2026-07-22T00:00:00Z',
    state: 'completed',
  };
}

afterEach(() => {
  cleanup();
});

describe('RunStatusBanner', () => {
  it('renders every provided completion metric with its unit', () => {
    render(
      <RunStatusBanner
        onOpenReview={vi.fn()}
        run={completedRun([
          { name: 'input_tokens', unit: 'tokens', value: 1234 },
          { name: 'thinking_duration_ms', unit: 'ms', value: 250 },
          { name: 'quality', unit: null, value: 0.95 },
        ])}
        taskId="tsk_000000000000000000000001"
        taskState="completed"
      />,
    );

    expect(screen.getByText('input_tokens: 1,234 tokens')).toBeVisible();
    expect(screen.getByText('thinking_duration_ms: 250 ms')).toBeVisible();
    expect(screen.getByText('quality: 0.95')).toBeVisible();
  });

  it('preserves null metrics as unavailable', () => {
    render(
      <RunStatusBanner
        onOpenReview={vi.fn()}
        run={completedRun(null)}
        taskId="tsk_000000000000000000000001"
        taskState="completed"
      />,
    );

    expect(screen.getByText('Metrics unavailable')).toBeVisible();
    expect(screen.queryByText(/0 tokens/)).toBeNull();
  });
});
