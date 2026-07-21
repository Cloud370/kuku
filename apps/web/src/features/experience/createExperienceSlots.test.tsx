import { describe, expect, it, vi } from 'vitest';

import { createExperienceSlots, type ExperienceSlotDependencies } from './createExperienceSlots';
import type { ReviewFileRouteState } from './presentationState';
import type { TaskQueryKind } from './taskDeltaEffects';

const taskId = 'tsk_000000000000000000000001';
const workspaceId = 'wsp_000000000000000000000001';

function dependencies(
  overrides: Partial<ExperienceSlotDependencies> = {},
): ExperienceSlotDependencies {
  return {
    enterReview: vi.fn(),
    invalidateTaskQuery: vi.fn<(taskId: string, query: TaskQueryKind) => void>(),
    readPresentation: () => ({
      chatScrollAnchor: { itemId: 'msg_0001', offsetPx: 12 },
      openContextSections: ['skills'],
      selectedRequestId: 'req_000000000000000000000002',
    }),
    readScope: () => ({ taskId, workspaceId }),
    restorePresentation: vi.fn(),
    ...overrides,
  };
}

describe('createExperienceSlots', () => {
  it('creates one stable relative-file callback and enters Review with Task presentation', () => {
    const deps = dependencies();
    const slots = createExperienceSlots(deps);

    expect(slots.onOpenFile).toBe(slots.onOpenFile);
    slots.onOpenFile(workspaceId, 'src/lib.rs');

    expect(deps.enterReview).toHaveBeenCalledWith({
      mode: 'files',
      relativePath: 'src/lib.rs',
      returnTo: {
        presentation: deps.readPresentation(taskId),
        taskId,
      },
      workspaceId,
    });
  });

  it.each(['/etc/passwd', '../src/lib.rs', 'C:\\src\\lib.rs', '', '  '])(
    'rejects unsafe relative path %j before navigation',
    (relativePath) => {
      const deps = dependencies();
      const slots = createExperienceSlots(deps);

      expect(() => {
        slots.onOpenFile(workspaceId, relativePath);
      }).toThrow();
      expect(deps.enterReview).not.toHaveBeenCalled();
    },
  );

  it('rejects a file belonging to another Workspace', () => {
    const deps = dependencies();
    const slots = createExperienceSlots(deps);

    expect(() => {
      slots.onOpenFile('wsp_000000000000000000000002', 'src/lib.rs');
    }).toThrow('another Workspace');
  });

  it('restores the saved Task presentation when leaving Review', () => {
    const restorePresentation = vi.fn();
    const slots = createExperienceSlots(dependencies({ restorePresentation }));
    const route = slots.openFile(workspaceId, 'src/lib.rs');

    slots.leaveReview(route);

    expect(restorePresentation).toHaveBeenCalledWith(taskId, route.returnTo.presentation);
  });

  it('keeps callback identity while reading the current Workbench scope', () => {
    let scope = { taskId, workspaceId };
    const enterReview = vi.fn<(state: ReviewFileRouteState) => void>();
    const slots = createExperienceSlots(
      dependencies({
        enterReview,
        readScope: () => scope,
      }),
    );
    const callback = slots.onOpenFile;

    scope = {
      taskId: 'tsk_000000000000000000000002',
      workspaceId: 'wsp_000000000000000000000002',
    };
    callback(scope.workspaceId, 'src/main.rs');

    expect(slots.onOpenFile).toBe(callback);
    const call = enterReview.mock.calls[0];
    if (call === undefined) throw new Error('Review route was not entered');
    expect(call[0].returnTo.taskId).toBe(scope.taskId);
    expect(call[0].workspaceId).toBe(scope.workspaceId);
  });

  it('rejects navigation while no Task and Workspace are selected', () => {
    const slots = createExperienceSlots(dependencies({ readScope: () => null }));

    expect(() => {
      slots.onOpenFile(workspaceId, 'src/lib.rs');
    }).toThrow('active Task');
  });

  it('exposes coalesced Task-query effects through the Workbench delta callback', () => {
    const invalidateTaskQuery = vi.fn<(taskId: string, query: TaskQueryKind) => void>();
    const slots = createExperienceSlots(dependencies({ invalidateTaskQuery }));

    slots.onTaskDeltaCommitted(taskId, {
      changes: [
        { context_summary: null, type: 'context_summary_changed' },
        { context_summary: null, type: 'context_summary_changed' },
      ],
      timeline_window: null,
      type: 'changes_applied',
    });

    expect(invalidateTaskQuery.mock.calls).toEqual([
      [taskId, 'context'],
      [taskId, 'agent_threads'],
    ]);
  });
});
