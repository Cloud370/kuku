import { describe, expect, it, vi } from 'vitest';

import {
  ReviewNavigationBoundary,
  createPresentationStore,
  type TaskPresentationSnapshot,
} from './presentationState';

const presentation: TaskPresentationSnapshot = {
  chatScrollAnchor: { itemId: 'msg_8', offsetPx: 24 },
  openContextSections: ['skills', 'observations'],
  selectedRequestId: 'req_000000000000000000000002',
};

describe('ReviewNavigationBoundary', () => {
  it('selects a relative file and restores Task-keyed presentation on return', () => {
    const enterReview = vi.fn();
    const restore = vi.fn();
    const boundary = new ReviewNavigationBoundary({
      enterReview,
      presentationStore: createPresentationStore(),
      readPresentation: () => presentation,
      restorePresentation: restore,
      taskId: 'tsk_000000000000000000000001',
      workspaceId: 'wsp_000000000000000000000001',
    });

    const routeState = boundary.openFile('wsp_000000000000000000000001', 'src/lib.rs');
    expect(enterReview).toHaveBeenCalledWith(routeState);
    expect(routeState.returnTo.presentation).toEqual(presentation);
    boundary.leaveReview(routeState);
    expect(restore).toHaveBeenCalledWith(presentation);
  });

  it('does not restore another Task return state', () => {
    const boundary = new ReviewNavigationBoundary({
      enterReview: vi.fn(),
      presentationStore: createPresentationStore(),
      readPresentation: () => presentation,
      restorePresentation: vi.fn(),
      taskId: 'tsk_000000000000000000000001',
      workspaceId: 'wsp_000000000000000000000001',
    });
    expect(() => {
      boundary.leaveReview({
        mode: 'files',
        relativePath: 'src/lib.rs',
        returnTo: {
          presentation,
          taskId: 'tsk_000000000000000000000002',
        },
        workspaceId: 'wsp_000000000000000000000001',
      });
    }).toThrow('another Task');
  });
});
