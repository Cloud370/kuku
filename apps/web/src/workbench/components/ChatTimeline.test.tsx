import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import taskProjectionJson from '../../api/generated/fixtures/task_projection.json';
import type {
  ActivityKind,
  ActivityProjection,
  CompletionProjection,
  InteractionProjection,
  TaskProjection,
  TimelineItemProjection,
} from '../../api/generated';
import type { TimelineHistory } from '../state';

import { ChatTimeline, type ChatTimelineProps } from './ChatTimeline';

const taskId = 'tsk_000000000000000000000001';
const workspaceId = 'wsp_000000000000000000000001';

function projection(items: TimelineItemProjection[] = []): TaskProjection {
  const value = structuredClone(taskProjectionJson) as TaskProjection;
  value.timeline = items;
  return value;
}

function history(overrides: Partial<TimelineHistory> = {}): TimelineHistory {
  return {
    error: null,
    gapAfter: false,
    generation: 0,
    items: [],
    nextCursor: null,
    phase: 'idle',
    retainedBytes: 0,
    ...overrides,
  };
}

function message(
  id: string,
  text = 'Existing message',
  requestIds: string[] = [],
): TimelineItemProjection {
  return {
    type: 'message',
    item: {
      file_references: [],
      finalized: true,
      message_id: id,
      order_key: 1,
      request_ids: requestIds,
      role: 'agent',
      text,
    },
  };
}

function activity(kind: ActivityKind, id = `activity-${kind}`): TimelineItemProjection {
  const item: ActivityProjection = {
    activity_id: id,
    detail: 'Server supplied detail',
    file_references: [],
    kind,
    order_key: 2,
    status: 'running',
    title: kind === 'tool' ? 'tool' : 'Delegated Agent',
  };
  return { type: 'activity', item };
}

function interaction(id = 'int_000000000000000000000001'): TimelineItemProjection {
  const item: InteractionProjection = {
    choices: [
      { choice_id: 'allow', label: 'Allow' },
      { choice_id: 'deny', label: 'Deny' },
    ],
    interaction_id: id,
    order_key: 3,
    prompt: 'Allow file write?',
    selected_choice_id: null,
    status: 'pending',
  };
  return { type: 'interaction', item };
}

function props(overrides: Partial<ChatTimelineProps> = {}): ChatTimelineProps {
  const taskProjection = projection();
  return {
    loadOlder: vi.fn(),
    onOpenFile: vi.fn(),
    onOpenRequestContext: vi.fn(),
    onOpenReview: vi.fn(),
    onRespond: vi.fn(),
    onReturnToRecent: vi.fn(),
    projection: taskProjection,
    timelineHistory: history(),
    timelineItems: taskProjection.timeline,
    ...overrides,
  };
}

afterEach(() => {
  cleanup();
});

describe('ChatTimeline', () => {
  it.each([
    ['tool' as const, 'tool'],
    ['delegated_agent' as const, 'Delegated Agent'],
    ['system' as const, 'Delegated Agent'],
  ])('renders %s activity inline from the projection', (kind, label) => {
    const items = [activity(kind)];
    render(<ChatTimeline {...props({ projection: projection(items), timelineItems: items })} />);
    expect(screen.getByText(label)).toBeVisible();
  });

  it('renders interaction choices and Needs Attention status', async () => {
    const user = userEvent.setup();
    const onRespond = vi.fn();
    const items = [interaction()];
    const value = projection(items);
    value.task.state = 'needs_attention';
    render(<ChatTimeline {...props({ onRespond, projection: value, timelineItems: items })} />);

    expect(screen.getByRole('group', { name: 'Permission request' })).toBeVisible();
    expect(screen.getByText('Needs Attention')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Allow' }));
    expect(onRespond).toHaveBeenCalledWith(taskId, 'int_000000000000000000000001', 'allow');
  });

  it('does not invent a delegated Agent thread action without a canonical identity', () => {
    const items = [activity('delegated_agent')];
    render(<ChatTimeline {...props({ projection: projection(items), timelineItems: items })} />);

    expect(screen.queryByRole('button', { name: 'Open delegated Agent thread' })).toBeNull();
  });

  it('keeps timeline row IDs distinct across projection kinds', () => {
    const items = [message('same-id'), activity('tool', 'same-id'), interaction('same-id')];
    render(<ChatTimeline {...props({ projection: projection(items), timelineItems: items })} />);

    expect(document.querySelector('[data-timeline-id="message:same-id"]')).not.toBeNull();
    expect(document.querySelector('[data-timeline-id="activity:same-id"]')).not.toBeNull();
    expect(document.querySelector('[data-timeline-id="interaction:same-id"]')).not.toBeNull();
  });

  it('preserves ordered Request IDs and opens historical Context', async () => {
    const user = userEvent.setup();
    const onOpenRequestContext = vi.fn();
    const items = [
      message('message-requests', 'Response', [
        'req_000000000000000000000001',
        'req_000000000000000000000002',
      ]),
    ];
    render(
      <ChatTimeline
        {...props({
          onOpenRequestContext,
          projection: projection(items),
          timelineItems: items,
        })}
      />,
    );

    await user.click(screen.getByRole('button', { name: 'Inspect Request context 2 of 2' }));
    expect(onOpenRequestContext).toHaveBeenCalledWith(taskId, 'req_000000000000000000000002');
  });

  it('expands tool detail and opens only typed file references', async () => {
    const user = userEvent.setup();
    const onOpenFile = vi.fn();
    const item = activity('tool');
    if (item.type !== 'activity') throw new Error('fixture must be activity');
    item.item.file_references = [
      { label: 'src/lib.rs', relative_path: 'src/lib.rs', workspace_id: workspaceId },
    ];
    const items = [item];
    render(
      <ChatTimeline
        {...props({ onOpenFile, projection: projection(items), timelineItems: items })}
      />,
    );

    await user.click(screen.getByRole('button', { name: 'Show tool details' }));
    expect(screen.getByRole('region', { name: 'Tool details' })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Open src/lib.rs' }));
    expect(onOpenFile).toHaveBeenCalledWith(workspaceId, 'src/lib.rs');
  });

  it('opens current Workspace Changes from available completion', async () => {
    const user = userEvent.setup();
    const onOpenReview = vi.fn();
    const value = projection();
    const completion: CompletionProjection = {
      checks: [{ detail: null, name: 'unit', passed: true }],
      metrics: [],
      summary: 'Done',
      warnings: [],
      workspace_changes: {
        api_version: 1,
        availability: 'available',
        entries: [],
        next_cursor: null,
        revision: 'revision-completion',
        workspace_id: workspaceId,
      },
    };
    value.latest_run = {
      completion,
      finished_at: '2026-07-18T00:01:00Z',
      run_id: 'run_000000000000000000000001',
      started_at: '2026-07-18T00:00:00Z',
      state: 'completed',
    };
    render(<ChatTimeline {...props({ onOpenReview, projection: value })} />);

    await user.click(screen.getByRole('button', { name: 'View changes' }));
    expect(onOpenReview).toHaveBeenCalledWith(taskId);
  });

  it('renders unavailable completion data without invented zero metrics', () => {
    const value = projection();
    value.latest_run = {
      completion: {
        checks: null,
        metrics: null,
        summary: 'Finished',
        warnings: [],
        workspace_changes: null,
      },
      finished_at: '2026-07-18T00:01:00Z',
      run_id: 'run_000000000000000000000001',
      started_at: '2026-07-18T00:00:00Z',
      state: 'completed',
    };
    render(<ChatTimeline {...props({ projection: value })} />);

    expect(screen.getByText('Checks unavailable')).toBeVisible();
    expect(screen.getByText('Workspace changes unavailable')).toBeVisible();
    expect(screen.queryByText('0 files')).toBeNull();
  });

  it('loads earlier messages, disables while loading, and hides at the beginning', async () => {
    const user = userEvent.setup();
    const loadOlder = vi.fn();
    const { rerender } = render(
      <ChatTimeline
        {...props({ loadOlder, timelineHistory: history({ nextCursor: 'page:first' }) })}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Load earlier messages' }));
    expect(loadOlder).toHaveBeenCalledTimes(1);

    rerender(
      <ChatTimeline
        {...props({
          loadOlder,
          timelineHistory: history({ nextCursor: 'page:first', phase: 'loading' }),
        })}
      />,
    );
    expect(screen.getByRole('button', { name: 'Load earlier messages' })).toBeDisabled();

    rerender(<ChatTimeline {...props({ timelineHistory: history({ nextCursor: null }) })} />);
    expect(screen.queryByRole('button', { name: 'Load earlier messages' })).toBeNull();
  });

  it('keeps prior items and exposes retry after an earlier-page failure', async () => {
    const user = userEvent.setup();
    const loadOlder = vi.fn();
    const items = [message('message-existing')];
    render(
      <ChatTimeline
        {...props({
          loadOlder,
          projection: projection(items),
          timelineHistory: history({ nextCursor: 'page:retry', phase: 'error' }),
          timelineItems: items,
        })}
      />,
    );

    expect(screen.getByText('Existing message')).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent('Earlier messages could not be loaded');
    await user.click(screen.getByRole('button', { name: 'Load earlier messages' }));
    expect(loadOlder).toHaveBeenCalledTimes(1);
  });

  it('renders a projection-free empty state', () => {
    render(<ChatTimeline {...props({ projection: null, timelineItems: [] })} />);
    expect(screen.getByText('Choose a Task to start chatting.')).toBeVisible();
  });
});
