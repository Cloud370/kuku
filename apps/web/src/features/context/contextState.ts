import type { ApiError, ContextCatalog, ContextSnapshot } from '../../api/generated';
import { WebApiError } from '../../api/client';

export type ContextPanelState =
  | { kind: 'loading' }
  | { kind: 'empty'; reason: 'no_request' | 'no_context' }
  | { kind: 'error'; error: ApiError }
  | {
      kind: 'ready';
      snapshot: ContextSnapshot;
      catalog: ContextCatalog;
      mode: 'verified' | 'preview';
    };

export function toApiError(error: unknown): ApiError {
  if (error instanceof WebApiError) {
    return {
      api_version: 1,
      code: error.code,
      message: error.message,
      trace_id: error.traceId,
      details: error.details,
    };
  }
  const value = error as Partial<ApiError & Error>;
  return {
    api_version: 1,
    code: value.code ?? 'internal',
    message: value.message ?? 'Context could not be loaded',
    trace_id: value.trace_id ?? 'context-client',
    details: value.details ?? null,
  };
}
