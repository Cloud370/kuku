import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ReviewSubmissionPage, TaskProjection } from '@/api/generated';

import { webApi } from '../../api/client';
import { catalogFixture, contextFixture, requestOne, requestTwo } from '../context/testFixtures';

import { createExperienceSlots } from './createExperienceSlots';
import type { ReviewFileRouteState } from './presentationState';

const taskId = 'tsk_000000000000000000000001';
const workspaceId = 'wsp_000000000000000000000001';
const presentation = {
  chatScrollAnchor: { itemId: 'msg_000000000000000000000001', offsetPx: 24 },
  openContextSections: ['skills', 'agents'],
  selectedRequestId: 'req_000000000000000000000002',
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('experience journeys', () => {
  it('navigates one response across two historical Requests with exact server hashes', async () => {
    const base = contextFixture();
    vi.spyOn(webApi.context, 'current').mockResolvedValue(base);
    vi.spyOn(webApi.context, 'historical').mockImplementation((_taskId, requestId) => {
      const selected = base.request_history.find((request) => request.request_id === requestId);
      if (selected === undefined) throw new Error('Request fixture is missing');
      return Promise.resolve(
        contextFixture({
          exact_payload_hash: `sha256:${requestId}`,
          selected_request: selected,
        }),
      );
    });
    vi.spyOn(webApi.catalog, 'workspace').mockResolvedValue(catalogFixture());
    const slots = createExperienceSlots({
      enterReview: vi.fn(),
      invalidateTaskQuery: vi.fn(),
      readPresentation: () => presentation,
      readScope: () => ({ taskId, workspaceId }),
      restorePresentation: vi.fn(),
    });
    const Context = slots.components.ContextPanel;
    function ContextJourney() {
      const [selectedRequestId, setSelectedRequestId] = useState<string | null>(null);
      return (
        <Context
          onOpenAgent={vi.fn()}
          onOpenFile={slots.context.onOpenFile}
          onOpenSectionsChange={vi.fn()}
          onSelectRequest={setSelectedRequestId}
          onStageSkill={vi.fn()}
          onUnstageSkill={vi.fn()}
          openSections={['skills']}
          selectedRequestId={selectedRequestId}
          stagedSkillIds={[]}
          taskId={taskId}
          workspaceId={workspaceId}
        />
      );
    }
    render(<ContextJourney />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: `Select Request ${requestOne}` }));
    expect(await screen.findByText('Historical Request')).toBeVisible();
    await user.click(screen.getByRole('button', { name: `Select Request ${requestTwo}` }));
    await user.click(await screen.findByRole('button', { name: 'View exact Request' }));

    expect(screen.getByText(`sha256:${requestTwo}`)).toBeVisible();
    expect(webApi.context.historical).toHaveBeenNthCalledWith(1, taskId, requestOne);
    expect(webApi.context.historical).toHaveBeenNthCalledWith(2, taskId, requestTwo);
  });

  it('keeps a submitted review visible after remount from the server projection', async () => {
    const page: ReviewSubmissionPage = {
      api_version: 1,
      items: [
        {
          notes: [
            {
              comment: 'Keep the exact server projection',
              end_line: 8,
              excerpt: 'server note',
              path: 'src/lib.rs',
              revision: 'a'.repeat(64),
              side: 'new',
              start_line: 8,
              status: 'current',
            },
          ],
          run_id: 'run_000000000000000000000001',
          submission_id: 'rsub_000000000000000000000001',
          submitted_at: '2026-07-21T00:00:00Z',
          task_id: taskId,
          task_revision: 8,
        },
      ],
      next_cursor: null,
      task_id: taskId,
    };
    const source = { submissions: vi.fn().mockResolvedValue(page) };
    const slots = createExperienceSlots({
      enterReview: vi.fn(),
      invalidateTaskQuery: vi.fn(),
      readPresentation: () => presentation,
      readScope: () => ({ taskId, workspaceId }),
      restorePresentation: vi.fn(),
    });
    const Submitted = slots.components.SubmittedReviewNotes;
    const first = render(<Submitted refreshKey={0} source={source} taskId={taskId} />);
    expect(await screen.findByText('Keep the exact server projection')).toBeVisible();

    first.unmount();
    render(<Submitted refreshKey={0} source={source} taskId={taskId} />);

    expect(await screen.findByText('Keep the exact server projection')).toBeVisible();
    expect(source.submissions).toHaveBeenCalledTimes(2);
  });

  it.each(['chat_path', 'context_observation'] as const)(
    'opens %s through the same typed Review Files route and returns',
    (source) => {
      const enterReview = vi.fn<(state: ReviewFileRouteState) => void>();
      const restorePresentation = vi.fn();
      const slots = createExperienceSlots({
        enterReview,
        invalidateTaskQuery: vi.fn(),
        readPresentation: () => presentation,
        readScope: () => ({ taskId, workspaceId }),
        restorePresentation,
      });
      const callback = source === 'chat_path' ? slots.chat.onOpenFile : slots.context.onOpenFile;

      callback(workspaceId, 'src/lib.rs');

      expect(callback).toBe(slots.onOpenFile);
      const call = enterReview.mock.calls[0];
      if (call === undefined) throw new Error('Review route was not entered');
      const [route] = call;
      expect(route).toMatchObject({
        mode: 'files',
        relativePath: 'src/lib.rs',
        returnTo: { taskId },
        workspaceId,
      });
      slots.leaveReview(route);
      expect(restorePresentation).toHaveBeenCalledWith(taskId, presentation);
    },
  );

  it('refreshes Context, threads, and Review truth after relevant batches and replacement', () => {
    const invalidate = vi.fn();
    const slots = createExperienceSlots({
      enterReview: vi.fn(),
      invalidateTaskQuery: invalidate,
      readPresentation: () => presentation,
      readScope: () => ({ taskId, workspaceId }),
      restorePresentation: vi.fn(),
    });

    slots.onTaskDeltaCommitted(taskId, {
      changes: [
        { context_summary: null, type: 'context_summary_changed' },
        {
          change: {
            submission: {
              notes: [],
              run_id: 'run_000000000000000000000001',
              submission_id: 'rsub_000000000000000000000001',
              submitted_at: '2026-07-21T00:00:00Z',
              task_id: taskId,
              task_revision: 1,
            },
            total_submissions: 1,
          },
          type: 'review_submissions_changed',
        },
      ],
      timeline_window: null,
      type: 'changes_applied',
    });
    slots.onTaskDeltaCommitted(taskId, {
      projection: {} as TaskProjection,
      type: 'projection_replaced',
    });

    expect(invalidate.mock.calls).toEqual(
      expect.arrayContaining([
        [taskId, 'context'],
        [taskId, 'agent_threads'],
        [taskId, 'review_submissions'],
        [taskId, 'historical_context'],
      ]),
    );
  });
});
