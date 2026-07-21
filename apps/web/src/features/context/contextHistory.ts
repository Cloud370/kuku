import type { RequestId } from '../../api/generated';

export interface ContextHistoryState {
  selectedRequestId: RequestId | null;
  mode: 'current' | 'verified';
  stagingTarget: 'latest';
}

export type ContextHistoryAction = { type: 'select'; requestId: RequestId } | { type: 'current' };

export function reduceContextHistory(
  state: ContextHistoryState,
  action: ContextHistoryAction,
): ContextHistoryState {
  if (action.type === 'select') {
    return {
      ...state,
      selectedRequestId: action.requestId,
      mode: 'verified',
      stagingTarget: 'latest',
    };
  }
  return {
    ...state,
    selectedRequestId: null,
    mode: 'current',
    stagingTarget: 'latest',
  };
}
