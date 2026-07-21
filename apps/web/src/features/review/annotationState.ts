import type {
  AnnotationBatch,
  AnnotationDraft,
  ApiError,
  ReviewSubmissionResult,
  TaskId,
  TaskRevision,
} from '@/api/generated';
import { WebApiError, webApi } from '../../api/client';

export type LocalReviewNote = {
  localId: string;
  note: AnnotationDraft;
  state: 'draft' | 'outdated' | 'submitting' | 'error';
  error: ApiError | null;
};

export type PendingReviewBatch = {
  taskId: TaskId;
  localIds: readonly string[];
  batch: AnnotationBatch;
};

export type SubmissionConflictAction =
  | 'mark_outdated'
  | 'refresh_task'
  | 'return_to_active_run'
  | 'abandon_batch'
  | 'retry';

type SubmitReviewSource = Pick<typeof webApi.review, 'submit'>;

export function makeLocalDraft(
  note: AnnotationDraft,
  localId: string = crypto.randomUUID(),
): LocalReviewNote {
  return {
    error: null,
    localId,
    note: { ...note },
    state: 'draft',
  };
}

export function toAnnotationDraft(local: LocalReviewNote): AnnotationDraft {
  return { ...local.note };
}

export function createPendingBatch(
  task: { taskId: TaskId; taskRevision: TaskRevision },
  notes: readonly AnnotationDraft[],
  createKey: () => string = () => crypto.randomUUID(),
): PendingReviewBatch {
  const wireNotes = notes.map((note) => Object.freeze({ ...note }));
  const batch: AnnotationBatch = {
    expected_task_revision: task.taskRevision,
    idempotency_key: createKey(),
    notes: wireNotes,
  };
  Object.freeze(batch.notes);
  Object.freeze(batch);
  return Object.freeze({
    batch,
    localIds: Object.freeze([]),
    taskId: task.taskId,
  });
}

export function createPendingBatchForLocalNotes(
  task: { taskId: TaskId; taskRevision: TaskRevision },
  notes: readonly LocalReviewNote[],
  createKey?: () => string,
): PendingReviewBatch {
  const command = createPendingBatch(task, notes.map(toAnnotationDraft), createKey);
  return Object.freeze({
    ...command,
    localIds: Object.freeze(notes.map((note) => note.localId)),
  });
}

export function retryPendingBatch(
  source: SubmitReviewSource,
  pending: PendingReviewBatch,
): Promise<ReviewSubmissionResult> {
  return source.submit(pending.taskId, pending.batch);
}

export function clearConfirmedDrafts(
  notes: readonly LocalReviewNote[],
  pending: PendingReviewBatch,
): LocalReviewNote[] {
  const confirmed = new Set(pending.localIds);
  return notes.filter((note) => !confirmed.has(note.localId));
}

export function classifySubmissionError(error: unknown): SubmissionConflictAction {
  if (!(error instanceof WebApiError)) return 'retry';
  if (error.code === 'outdated') return 'mark_outdated';
  if (error.code === 'stale_command') return 'refresh_task';
  if (error.code === 'task_busy') return 'return_to_active_run';
  if (error.code === 'idempotency_conflict') return 'abandon_batch';
  return 'retry';
}
