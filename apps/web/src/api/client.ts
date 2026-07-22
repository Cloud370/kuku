import type {
  AgentThread,
  AnnotationBatch,
  ApiError,
  CatalogQuery,
  ChangesQuery,
  CommandAccepted,
  CompleteInitRequest,
  ContextCatalog,
  ContextSnapshot,
  ConversationId,
  CreateTaskRequest,
  CreateTaskResponse,
  Cursor,
  DiffDocument,
  DiffQuery,
  FileContent,
  FileContentQuery,
  FilePage,
  FileSearchPage,
  FileSearchQuery,
  FileTreeQuery,
  InitStatus,
  InteractionId,
  InteractionResponseRequest,
  ListTasksQuery,
  PlatformCatalog,
  PlatformStatus,
  RegisterInitialWorkspaceRequest,
  RegisterWorkspaceRequest,
  RegistrationRootPage,
  RemoveWorkspaceRequest,
  RequestId,
  ReviewSubmissionPage,
  ReviewSubmissionQuery,
  ReviewSubmissionResult,
  ReviewSnapshot,
  SettingsSnapshot,
  StopRunRequest,
  SubmitRunRequest,
  SubmitRunResponse,
  TaskId,
  TaskPage,
  TaskProjection,
  TestProviderRequest,
  TestProviderResult,
  TimelinePage,
  TimelineQuery,
  UpdateDefaultTierRequest,
  UpdateProvidersRequest,
  UpdateSettingsRequest,
  WorkspaceId,
  WorkspacePage,
  WorkspaceSummary,
} from "./generated";
import { decodeApiError } from "./decode";

const API_ROOT = "/api/v1";
const CREDENTIAL_STORAGE_KEY = "kuku.web.credential";

function credentialStorage(): Storage | null {
  return typeof localStorage === "undefined" ? null : localStorage;
}

const credentials = {
  set(value: string): void {
    credentialStorage()?.setItem(CREDENTIAL_STORAGE_KEY, value);
  },
  clear(): void {
    credentialStorage()?.removeItem(CREDENTIAL_STORAGE_KEY);
  },
  current(): string | null {
    return credentialStorage()?.getItem(CREDENTIAL_STORAGE_KEY) ?? null;
  },
};

export class WebApiError extends Error {
  readonly status: number;
  readonly code: ApiError["code"];
  readonly traceId: string;
  readonly details: ApiError["details"];

  constructor(status: number, error: ApiError) {
    super(error.message);
    this.name = "WebApiError";
    this.status = status;
    this.code = error.code;
    this.traceId = error.trace_id;
    this.details = error.details;
  }
}

async function throwResponseError(response: Response): Promise<never> {
  let value: unknown;
  try {
    value = await response.json();
  } catch {
    throw new Error(`HTTP ${String(response.status)} returned a non-JSON error`);
  }
  throw new WebApiError(response.status, decodeApiError(value));
}

function requestHeaders(hasBody: boolean, accept: string): Headers {
  const headers = new Headers({ Accept: accept });
  if (hasBody) headers.set("Content-Type", "application/json");
  const credential = credentials.current();
  if (credential !== null) headers.set("Authorization", `Bearer ${credential}`);
  return headers;
}

async function requestResponse(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal,
  accept = "application/json",
): Promise<Response> {
  const hasBody = body !== undefined;
  const response = await fetch(path, {
    method,
    headers: requestHeaders(hasBody, accept),
    ...(hasBody ? { body: JSON.stringify(body) } : {}),
    ...(signal === undefined ? {} : { signal }),
  });
  if (!response.ok) await throwResponseError(response);
  return response;
}

async function requestJson<T>(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  const response = await requestResponse(method, path, body, signal);
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

function queryPath(path: string, values: object): string {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value === null || value === undefined) continue;
    if (typeof value !== "string" && typeof value !== "number" && typeof value !== "boolean") {
      throw new TypeError(`query parameter ${key} must be scalar`);
    }
    query.set(key, String(value));
  }
  const encoded = query.toString();
  return encoded.length === 0 ? path : `${path}?${encoded}`;
}

function segment(value: string): string {
  return encodeURIComponent(value);
}

export const webApi = {
  platform: {
    status: (signal?: AbortSignal): Promise<PlatformStatus> =>
      requestJson("GET", `${API_ROOT}/status`, undefined, signal),
  },
  init: {
    status: (): Promise<InitStatus> => requestJson("GET", `${API_ROOT}/init/status`),
    providers: (body: UpdateProvidersRequest): Promise<InitStatus> =>
      requestJson("POST", `${API_ROOT}/init/providers`, body),
    defaultTier: (body: UpdateDefaultTierRequest): Promise<InitStatus> =>
      requestJson("POST", `${API_ROOT}/init/default-tier`, body),
    workspace: (body: RegisterInitialWorkspaceRequest): Promise<InitStatus> =>
      requestJson("POST", `${API_ROOT}/init/workspace`, body),
    test: (body: TestProviderRequest): Promise<TestProviderResult> =>
      requestJson("POST", `${API_ROOT}/init/test`, body),
    complete: (body: CompleteInitRequest): Promise<InitStatus> =>
      requestJson("POST", `${API_ROOT}/init/complete`, body),
  },
  workspaces: {
    registrationRoots: (): Promise<RegistrationRootPage> =>
      requestJson("GET", `${API_ROOT}/registration-roots`),
    list: (): Promise<WorkspacePage> => requestJson("GET", `${API_ROOT}/workspaces`),
    register: (body: RegisterWorkspaceRequest): Promise<WorkspaceSummary> =>
      requestJson("POST", `${API_ROOT}/workspaces`, body),
    remove: (workspaceId: WorkspaceId, body: RemoveWorkspaceRequest): Promise<void> =>
      requestJson("DELETE", `${API_ROOT}/workspaces/${segment(workspaceId)}`, body),
  },
  catalog: {
    platform: (): Promise<PlatformCatalog> => requestJson("GET", `${API_ROOT}/catalog`),
    workspace: (workspaceId: WorkspaceId, query: CatalogQuery): Promise<ContextCatalog> =>
      requestJson(
        "GET",
        queryPath(`${API_ROOT}/workspaces/${segment(workspaceId)}/catalog`, query),
      ),
  },
  tasks: {
    list: (query: ListTasksQuery): Promise<TaskPage> =>
      requestJson("GET", queryPath(`${API_ROOT}/tasks`, query)),
    create: (body: CreateTaskRequest): Promise<CreateTaskResponse> =>
      requestJson("POST", `${API_ROOT}/tasks`, body),
    get: (taskId: TaskId): Promise<TaskProjection> =>
      requestJson("GET", `${API_ROOT}/tasks/${segment(taskId)}`),
    timeline: (taskId: TaskId, query: TimelineQuery): Promise<TimelinePage> => {
      const before: unknown = query.before;
      if (before !== null && typeof before !== "string") {
        return Promise.reject(
          new TypeError("timeline before must be an opaque PageCursor string or null"),
        );
      }
      return requestJson(
        "GET",
        queryPath(`${API_ROOT}/tasks/${segment(taskId)}/timeline`, query),
      );
    },
    submitRun: (taskId: TaskId, body: SubmitRunRequest): Promise<SubmitRunResponse> =>
      requestJson("POST", `${API_ROOT}/tasks/${segment(taskId)}/runs`, body),
    stopRun: (taskId: TaskId, body: StopRunRequest): Promise<CommandAccepted> =>
      requestJson("POST", `${API_ROOT}/tasks/${segment(taskId)}/stop`, body),
    respond: (
      taskId: TaskId,
      interactionId: InteractionId,
      body: InteractionResponseRequest,
    ): Promise<CommandAccepted> =>
      requestJson(
        "POST",
        `${API_ROOT}/tasks/${segment(taskId)}/interactions/${segment(interactionId)}`,
        body,
      ),
    subscribe: async (
      taskId: TaskId,
      after: Cursor | null,
      signal: AbortSignal,
    ): Promise<Response> => {
      const response = await requestResponse(
        "GET",
        queryPath(`${API_ROOT}/tasks/${segment(taskId)}/stream`, { after }),
        undefined,
        signal,
        "application/x-ndjson",
      );
      if (response.body === null) {
        throw new Error("task subscription response has no body");
      }
      return response;
    },
  },
  context: {
    current: (taskId: TaskId): Promise<ContextSnapshot> =>
      requestJson("GET", `${API_ROOT}/tasks/${segment(taskId)}/context`),
    historical: (taskId: TaskId, requestId: RequestId): Promise<ContextSnapshot> =>
      requestJson(
        "GET",
        `${API_ROOT}/tasks/${segment(taskId)}/context/${segment(requestId)}`,
      ),
    agentThread: (taskId: TaskId, conversationId: ConversationId): Promise<AgentThread> =>
      requestJson(
        "GET",
        `${API_ROOT}/tasks/${segment(taskId)}/agents/${segment(conversationId)}`,
      ),
  },
  review: {
    tree: (workspaceId: WorkspaceId, query: FileTreeQuery): Promise<FilePage> =>
      requestJson(
        "GET",
        queryPath(`${API_ROOT}/workspaces/${segment(workspaceId)}/files/tree`, query),
      ),
    search: (workspaceId: WorkspaceId, query: FileSearchQuery): Promise<FileSearchPage> =>
      requestJson(
        "GET",
        queryPath(`${API_ROOT}/workspaces/${segment(workspaceId)}/files/search`, query),
      ),
    file: (workspaceId: WorkspaceId, query: FileContentQuery): Promise<FileContent> =>
      requestJson(
        "GET",
        queryPath(`${API_ROOT}/workspaces/${segment(workspaceId)}/files/content`, query),
      ),
    changes: (workspaceId: WorkspaceId, query: ChangesQuery): Promise<ReviewSnapshot> =>
      requestJson(
        "GET",
        queryPath(`${API_ROOT}/workspaces/${segment(workspaceId)}/changes`, query),
      ),
    diff: (workspaceId: WorkspaceId, query: DiffQuery): Promise<DiffDocument> =>
      requestJson(
        "GET",
        queryPath(`${API_ROOT}/workspaces/${segment(workspaceId)}/changes/diff`, query),
      ),
    submissions: (taskId: TaskId, query: ReviewSubmissionQuery): Promise<ReviewSubmissionPage> =>
      requestJson(
        "GET",
        queryPath(`${API_ROOT}/tasks/${segment(taskId)}/review/submissions`, query),
      ),
    submit: (taskId: TaskId, body: AnnotationBatch): Promise<ReviewSubmissionResult> =>
      requestJson("POST", `${API_ROOT}/tasks/${segment(taskId)}/review/annotations`, body),
  },
  settings: {
    get: (): Promise<SettingsSnapshot> => requestJson("GET", `${API_ROOT}/settings`),
    update: (body: UpdateSettingsRequest): Promise<SettingsSnapshot> =>
      requestJson("PATCH", `${API_ROOT}/settings`, body),
  },
  credentials,
} as const;
