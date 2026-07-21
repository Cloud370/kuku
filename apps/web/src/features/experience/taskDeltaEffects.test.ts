import { describe, expect, it, vi } from 'vitest';
import type { TaskChange, TaskDelta, TaskProjection } from '@/api/generated';

import { createTaskDeltaEffects } from './taskDeltaEffects';

const taskId = 'tsk_000000000000000000000001';

function changes(changes: TaskChange[]): TaskDelta {
  return { changes, timeline_window: null, type: 'changes_applied' };
}

describe('taskDeltaEffects', () => {
  it('coalesces relevant Context, Agent, and Review invalidations within one committed batch', () => {
    const invalidate = vi.fn();
    const effects = createTaskDeltaEffects({ invalidate });

    effects.commit(
      taskId,
      changes([
        { context_summary: null, type: 'context_summary_changed' },
        {
          activity: {
            activity_id: 'act_000000000000000000000001',
            detail: null,
            file_references: [],
            kind: 'delegated_agent',
            order_key: 1,
            status: 'completed',
            title: 'Research',
          },
          type: 'activity_upserted',
        },
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
            total_submissions: 2,
          },
          type: 'review_submissions_changed',
        },
        { context_summary: null, type: 'context_summary_changed' },
      ]),
    );

    expect(invalidate.mock.calls).toEqual([
      [taskId, 'context'],
      [taskId, 'agent_threads'],
      [taskId, 'review_submissions'],
    ]);
  });

  it('invalidates all dependent projections after a lag replacement', () => {
    const invalidate = vi.fn();
    const effects = createTaskDeltaEffects({ invalidate });

    effects.commit(taskId, {
      projection: {} as TaskProjection,
      type: 'projection_replaced',
    });

    expect(invalidate.mock.calls).toEqual([
      [taskId, 'context'],
      [taskId, 'historical_context'],
      [taskId, 'agent_threads'],
      [taskId, 'review_submissions'],
    ]);
  });

  it('does not invalidate per streaming token before a message is finalized', () => {
    const invalidate = vi.fn();
    const effects = createTaskDeltaEffects({ invalidate });

    effects.commit(
      taskId,
      changes([
        {
          append_text: 'token',
          finalized: false,
          message_id: 'msg_000000000000000000000001',
          request_ids: null,
          type: 'message_patched',
        },
      ]),
    );

    expect(invalidate).not.toHaveBeenCalled();
  });

  it('refreshes Context and Agent truth after a finalized message', () => {
    const invalidate = vi.fn();
    const effects = createTaskDeltaEffects({ invalidate });

    effects.commit(
      taskId,
      changes([
        {
          append_text: '',
          finalized: true,
          message_id: 'msg_000000000000000000000001',
          request_ids: ['req_000000000000000000000002'],
          type: 'message_patched',
        },
      ]),
    );

    expect(invalidate.mock.calls).toEqual([
      [taskId, 'context'],
      [taskId, 'agent_threads'],
    ]);
  });

  it('refreshes Context and Agent truth for an appended finalized message', () => {
    const invalidate = vi.fn();
    const effects = createTaskDeltaEffects({ invalidate });

    effects.commit(
      taskId,
      changes([
        {
          item: {
            item: {
              file_references: [],
              finalized: true,
              message_id: 'msg_000000000000000000000001',
              order_key: 1,
              request_ids: ['req_000000000000000000000002'],
              role: 'agent',
              text: 'Done',
            },
            type: 'message',
          },
          type: 'message_appended',
        },
      ]),
    );

    expect(invalidate.mock.calls).toEqual([
      [taskId, 'context'],
      [taskId, 'agent_threads'],
    ]);
  });
});
