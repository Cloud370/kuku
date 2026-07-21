import { describe, expect, it, vi } from 'vitest';
import type {
  AnnotationDraft,
  AnnotationBatch,
  ReviewSubmissionResult,
  TaskId,
} from '@/api/generated';
import { WebApiError } from '../../api/client';

import {
  classifySubmissionError,
  clearConfirmedDrafts,
  createPendingBatch,
  createPendingBatchForLocalNotes,
  makeLocalDraft,
  retryPendingBatch,
  toAnnotationDraft,
} from './annotationState';
import { validateNote } from './annotationValidation';

const taskId: TaskId = 'tsk_000000000000000000000001';
const revisionA = 'a'.repeat(64);
const revisionB = 'b'.repeat(64);

function note(overrides: Partial<AnnotationDraft> = {}): AnnotationDraft {
  return {
    comment: 'Please rename this',
    end_line: 1,
    excerpt: 'const old = 1;',
    path: 'src/lib.ts',
    revision: revisionA,
    side: 'old',
    start_line: 1,
    ...overrides,
  };
}

describe('annotation state', () => {
  it('preserves side and marks a changed anchor outdated', () => {
    const local = makeLocalDraft(note(), 'local-note');
    expect(toAnnotationDraft(local).side).toBe('old');
    expect(
      validateNote(local, {
        api_version: 1,
        binary: false,
        end_line: 1,
        next_start_line: null,
        path: 'src/lib.ts',
        revision: revisionB,
        start_line: 1,
        text: 'const old = 1;',
        total_lines: 1,
        truncated: false,
        workspace_id: 'wsp_000000000000000000000001',
      }),
    ).toEqual({ kind: 'outdated' });
  });

  it('retries one immutable logical batch with a stable identity', async () => {
    const result = {
      api_version: 1,
      replayed: false,
      submission: {
        notes: [],
        run_id: 'run_000000000000000000000001',
        submission_id: 'rsub_000000000000000000000001',
        submitted_at: '2026-07-21T00:00:00Z',
        task_id: taskId,
        task_revision: 8,
      },
    } satisfies ReviewSubmissionResult;
    const submit = vi
      .fn<(taskId: TaskId, batch: AnnotationBatch) => Promise<ReviewSubmissionResult>>()
      .mockResolvedValue(result);
    const notes = [
      note(),
      note({ path: 'src/main.ts', side: 'new' }),
      note({ path: 'README.md', side: 'file' }),
    ];
    const command = createPendingBatch(
      { taskId, taskRevision: 7 },
      notes,
      () => 'stable-retry-key',
    );

    await retryPendingBatch({ submit }, command);
    await retryPendingBatch({ submit }, command);

    expect(submit).toHaveBeenCalledTimes(2);
    expect(submit.mock.calls[0]?.[1]).toBe(command.batch);
    expect(submit.mock.calls[1]?.[1]).toBe(command.batch);
    expect(submit.mock.calls[0]?.[1].idempotency_key).toBe('stable-retry-key');
    expect(Object.isFrozen(command.batch)).toBe(true);
  });

  it.each([
    ['outdated', 'mark_outdated'],
    ['stale_command', 'refresh_task'],
    ['task_busy', 'return_to_active_run'],
    ['idempotency_conflict', 'abandon_batch'],
  ] as const)('classifies %s without discarding drafts', (code, expected) => {
    const error = new WebApiError(409, {
      api_version: 1,
      code,
      details: null,
      message: code,
      trace_id: 'review-test',
    });
    expect(classifySubmissionError(error)).toBe(expected);
  });

  it('clears only drafts confirmed by the successful logical batch', () => {
    const first = makeLocalDraft(note(), 'first');
    const second = makeLocalDraft(note({ path: 'src/main.ts' }), 'second');
    const pending = createPendingBatchForLocalNotes(
      { taskId, taskRevision: 7 },
      [first],
      () => 'confirmed-key',
    );

    expect(clearConfirmedDrafts([first, second], pending).map((local) => local.localId)).toEqual([
      'second',
    ]);
  });
});
