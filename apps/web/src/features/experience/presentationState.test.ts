import { describe, expect, it } from 'vitest';

import {
  createPresentationStore,
  createReviewFileRouteState,
  emptyTaskPresentation,
} from './presentationState';

const taskId = 'tsk_000000000000000000000001';
const workspaceId = 'wsp_000000000000000000000001';

describe('presentation state', () => {
  it('keeps presentation state isolated by Task', () => {
    const store = createPresentationStore();
    store.save(taskId, {
      chatScrollAnchor: { itemId: 'msg_8', offsetPx: 24 },
      openContextSections: ['skills'],
      selectedRequestId: 'req_000000000000000000000002',
    });

    expect(store.read('tsk_000000000000000000000002')).toEqual(emptyTaskPresentation());
  });

  it.each(['/src/lib.rs', '../src/lib.rs', 'C:\\src\\lib.rs', 'C:src\\lib.rs', '', '   '])(
    'rejects unsafe Review file path %j',
    (relativePath) => {
      expect(() =>
        createReviewFileRouteState({
          expectedWorkspaceId: workspaceId,
          presentation: emptyTaskPresentation(),
          relativePath,
          taskId,
          workspaceId,
        }),
      ).toThrow();
    },
  );

  it('rejects a file from another Workspace', () => {
    expect(() =>
      createReviewFileRouteState({
        expectedWorkspaceId: workspaceId,
        presentation: emptyTaskPresentation(),
        relativePath: 'src/lib.rs',
        taskId,
        workspaceId: 'wsp_000000000000000000000002',
      }),
    ).toThrow('another Workspace');
  });
});
