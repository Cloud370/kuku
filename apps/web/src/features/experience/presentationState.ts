import type { RequestId, TaskId, WorkspaceId } from '../../api/generated';

export interface ChatScrollAnchor {
  itemId: string;
  offsetPx: number;
}

export interface TaskPresentationSnapshot {
  chatScrollAnchor: ChatScrollAnchor | null;
  selectedRequestId: RequestId | null;
  openContextSections: string[];
}

export interface ReviewFileRouteState {
  mode: 'files';
  workspaceId: WorkspaceId;
  relativePath: string;
  returnTo: {
    taskId: TaskId;
    presentation: TaskPresentationSnapshot;
  };
}

export function emptyTaskPresentation(): TaskPresentationSnapshot {
  return {
    chatScrollAnchor: null,
    openContextSections: [],
    selectedRequestId: null,
  };
}

export interface PresentationStore {
  read(taskId: TaskId): TaskPresentationSnapshot;
  save(taskId: TaskId, presentation: TaskPresentationSnapshot): void;
}

export function createPresentationStore(): PresentationStore {
  const entries = new Map<TaskId, TaskPresentationSnapshot>();
  return {
    read(taskId) {
      return clonePresentation(entries.get(taskId) ?? emptyTaskPresentation());
    },
    save(taskId, presentation) {
      entries.set(taskId, clonePresentation(presentation));
    },
  };
}

interface ReviewFileRouteInput {
  expectedWorkspaceId: WorkspaceId;
  presentation: TaskPresentationSnapshot;
  relativePath: string;
  taskId: TaskId;
  workspaceId: WorkspaceId;
}

export function createReviewFileRouteState(input: ReviewFileRouteInput): ReviewFileRouteState {
  if (input.workspaceId !== input.expectedWorkspaceId) {
    throw new Error('Review file belongs to another Workspace');
  }
  validateRelativePath(input.relativePath);
  return {
    mode: 'files',
    relativePath: input.relativePath,
    returnTo: {
      presentation: clonePresentation(input.presentation),
      taskId: input.taskId,
    },
    workspaceId: input.workspaceId,
  };
}

interface ReviewNavigationBoundaryOptions {
  enterReview: (state: ReviewFileRouteState) => void;
  presentationStore: PresentationStore;
  readPresentation: () => TaskPresentationSnapshot;
  restorePresentation: (presentation: TaskPresentationSnapshot) => void;
  taskId: TaskId;
  workspaceId: WorkspaceId;
}

export class ReviewNavigationBoundary {
  readonly #options: ReviewNavigationBoundaryOptions;

  constructor(options: ReviewNavigationBoundaryOptions) {
    this.#options = options;
  }

  openFile(workspaceId: WorkspaceId, relativePath: string): ReviewFileRouteState {
    const presentation = this.#options.readPresentation();
    this.#options.presentationStore.save(this.#options.taskId, presentation);
    const state = createReviewFileRouteState({
      expectedWorkspaceId: this.#options.workspaceId,
      presentation,
      relativePath,
      taskId: this.#options.taskId,
      workspaceId,
    });
    this.#options.enterReview(state);
    return state;
  }

  leaveReview(state: ReviewFileRouteState): void {
    if (state.returnTo.taskId !== this.#options.taskId) {
      throw new Error('Review return state belongs to another Task');
    }
    const presentation = clonePresentation(state.returnTo.presentation);
    this.#options.presentationStore.save(this.#options.taskId, presentation);
    this.#options.restorePresentation(presentation);
  }
}

function validateRelativePath(relativePath: string): void {
  if (
    relativePath.trim().length === 0 ||
    relativePath.startsWith('/') ||
    relativePath.startsWith('\\') ||
    /^[A-Za-z][A-Za-z0-9+.-]*:/u.test(relativePath)
  ) {
    throw new Error('Review file path must be Workspace-relative');
  }
  const components = relativePath.split(/[\\/]/u);
  if (
    components.some(
      (component) => component.length === 0 || component === '.' || component === '..',
    )
  ) {
    throw new Error('Review file path contains an invalid component');
  }
}

function clonePresentation(presentation: TaskPresentationSnapshot): TaskPresentationSnapshot {
  return {
    chatScrollAnchor:
      presentation.chatScrollAnchor === null ? null : { ...presentation.chatScrollAnchor },
    openContextSections: [...presentation.openContextSections],
    selectedRequestId: presentation.selectedRequestId,
  };
}
