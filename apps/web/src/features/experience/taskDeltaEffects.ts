import type { TaskDelta, TaskId } from '@/api/generated';

export type TaskQueryKind =
  | 'context'
  | 'historical_context'
  | 'agent_threads'
  | 'review_submissions';

export interface TaskDeltaEffectsDependencies {
  invalidate: (taskId: TaskId, query: TaskQueryKind) => void;
}

export interface TaskDeltaEffects {
  commit: (taskId: TaskId, delta: TaskDelta) => void;
}

const contextAndThreads: readonly TaskQueryKind[] = ['context', 'agent_threads'];
const replacementQueries: readonly TaskQueryKind[] = [
  'context',
  'historical_context',
  'agent_threads',
  'review_submissions',
];

export function createTaskDeltaEffects(deps: TaskDeltaEffectsDependencies): TaskDeltaEffects {
  return {
    commit(taskId, delta) {
      const invalidations = new Set<TaskQueryKind>();
      if (delta.type === 'projection_replaced') {
        replacementQueries.forEach((query) => {
          invalidations.add(query);
        });
      } else {
        for (const change of delta.changes) {
          switch (change.type) {
            case 'context_summary_changed':
            case 'skills_changed':
            case 'run_state_changed':
              contextAndThreads.forEach((query) => {
                invalidations.add(query);
              });
              break;
            case 'message_patched':
              if (change.finalized) {
                contextAndThreads.forEach((query) => {
                  invalidations.add(query);
                });
              }
              break;
            case 'activity_upserted':
              if (change.activity.kind === 'delegated_agent') {
                contextAndThreads.forEach((query) => {
                  invalidations.add(query);
                });
              }
              break;
            case 'review_submissions_changed':
              invalidations.add('review_submissions');
              break;
            case 'message_appended':
              if (change.item.type === 'message' && change.item.item.finalized) {
                contextAndThreads.forEach((query) => {
                  invalidations.add(query);
                });
              }
              break;
            case 'interaction_upserted':
              break;
          }
        }
      }
      invalidations.forEach((query) => {
        deps.invalidate(taskId, query);
      });
    },
  };
}
