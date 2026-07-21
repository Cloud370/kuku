export enum GuideAction {
  Init = 'init',
  Settings = 'settings',
  SkillPicker = 'skill_picker',
  Context = 'context',
  AgentThread = 'agent_thread',
  Files = 'files',
  ReviewChanges = 'review_changes',
  ConnectionQr = 'connection_qr',
  FirstTask = 'first_task',
}

const routes: Record<GuideAction, string> = {
  [GuideAction.Init]: '/init',
  [GuideAction.Settings]: '/settings',
  [GuideAction.SkillPicker]: '/tasks/new?guide=skills',
  [GuideAction.Context]: '/tasks/new?guide=context',
  [GuideAction.AgentThread]: '/tasks/new?guide=agent',
  [GuideAction.Files]: '/tasks/new?guide=files',
  [GuideAction.ReviewChanges]: '/tasks/new?guide=review',
  [GuideAction.ConnectionQr]: '/settings?connection=qr',
  [GuideAction.FirstTask]: '/tasks/new',
};

export function guideActionRoute(action: GuideAction): string {
  return routes[action];
}

export interface FirstTaskPrefill {
  editable: true;
  message: string;
}

export const firstTaskPrefill: FirstTaskPrefill = {
  editable: true,
  message: 'Inspect this workspace and identify the most important next step.',
};
