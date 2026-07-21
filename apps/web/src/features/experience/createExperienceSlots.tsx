import { webApi } from '../../api/client';
import type { TaskDelta, TaskId, WorkspaceId } from '@/api/generated';
import { SafeCodeBlock } from '../../components/content/SafeCodeBlock';
import { SafeMarkdown } from '../../components/content/SafeMarkdown';
import { AgentThreadDialog } from '../context/AgentThreadDialog';
import { ContextPanel } from '../context/ContextPanel';
import { RequestLinks } from '../context/RequestLinks';
import { GuideRoute } from '../guide/GuideRoute';
import { AnnotationDrafts } from '../review/AnnotationDrafts';
import { ReviewRoute } from '../review/ReviewRoute';
import { SubmittedReviewNotes } from '../review/SubmittedReviewNotes';
import { SettingsRoute } from '../settings/SettingsRoute';
import { ThemeControl } from '../theme/ThemeControl';

import {
  createPresentationStore,
  createReviewFileRouteState,
  type PresentationStore,
  type ReviewFileRouteState,
  type TaskPresentationSnapshot,
} from './presentationState';
import { createTaskDeltaEffects, type TaskQueryKind } from './taskDeltaEffects';

export type RelativeFileNavigation = (workspaceId: WorkspaceId, relativePath: string) => void;

export interface ExperienceScope {
  taskId: TaskId;
  workspaceId: WorkspaceId;
}

export interface ExperienceSlotDependencies {
  enterReview: (state: ReviewFileRouteState) => void;
  invalidateTaskQuery: (taskId: TaskId, query: TaskQueryKind) => void;
  presentationStore?: PresentationStore;
  readPresentation: (taskId: TaskId) => TaskPresentationSnapshot;
  readScope: () => ExperienceScope | null;
  restorePresentation: (taskId: TaskId, presentation: TaskPresentationSnapshot) => void;
}

export interface ExperienceSlots {
  api: typeof webApi;
  chat: { onOpenFile: RelativeFileNavigation };
  components: {
    AgentThreadDialog: typeof AgentThreadDialog;
    AnnotationDrafts: typeof AnnotationDrafts;
    ContextPanel: typeof ContextPanel;
    GuideRoute: typeof GuideRoute;
    RequestLinks: typeof RequestLinks;
    ReviewRoute: typeof ReviewRoute;
    SafeCodeBlock: typeof SafeCodeBlock;
    SafeMarkdown: typeof SafeMarkdown;
    SettingsRoute: typeof SettingsRoute;
    SubmittedReviewNotes: typeof SubmittedReviewNotes;
    ThemeControl: typeof ThemeControl;
  };
  context: { onOpenFile: RelativeFileNavigation };
  leaveReview: (state: ReviewFileRouteState) => void;
  onOpenFile: RelativeFileNavigation;
  onTaskDeltaCommitted: (taskId: TaskId, delta: TaskDelta) => void;
  openFile: (workspaceId: WorkspaceId, relativePath: string) => ReviewFileRouteState;
  presentationStore: PresentationStore;
}

export function createExperienceSlots(deps: ExperienceSlotDependencies): ExperienceSlots {
  const presentationStore = deps.presentationStore ?? createPresentationStore();
  const taskDeltaEffects = createTaskDeltaEffects({ invalidate: deps.invalidateTaskQuery });
  const openFile = (workspaceId: WorkspaceId, relativePath: string): ReviewFileRouteState => {
    const scope = deps.readScope();
    if (scope === null) throw new Error('Review navigation requires an active Task and Workspace');
    const presentation = deps.readPresentation(scope.taskId);
    const state = createReviewFileRouteState({
      expectedWorkspaceId: scope.workspaceId,
      presentation,
      relativePath,
      taskId: scope.taskId,
      workspaceId,
    });
    presentationStore.save(scope.taskId, presentation);
    deps.enterReview(state);
    return state;
  };
  const onOpenFile: RelativeFileNavigation = (workspaceId, relativePath) => {
    openFile(workspaceId, relativePath);
  };

  return {
    api: webApi,
    chat: { onOpenFile },
    components: {
      AgentThreadDialog,
      AnnotationDrafts,
      ContextPanel,
      GuideRoute,
      RequestLinks,
      ReviewRoute,
      SafeCodeBlock,
      SafeMarkdown,
      SettingsRoute,
      SubmittedReviewNotes,
      ThemeControl,
    },
    context: { onOpenFile },
    leaveReview: (state) => {
      const scope = deps.readScope();
      if (scope === null || state.returnTo.taskId !== scope.taskId) {
        throw new Error('Review return state belongs to another Task');
      }
      presentationStore.save(scope.taskId, state.returnTo.presentation);
      deps.restorePresentation(scope.taskId, presentationStore.read(scope.taskId));
    },
    onOpenFile,
    onTaskDeltaCommitted: taskDeltaEffects.commit,
    openFile,
    presentationStore,
  };
}
