import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { WebApiError, webApi } from "./client";
import { ContractDecodeError } from "./decode";

const revision = "a".repeat(64);
const taskId = "tsk_000000000000000000000001";
const workspaceId = "wsp_000000000000000000000001";

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function requestedPaths(fetchMock: ReturnType<typeof vi.fn<typeof fetch>>): string[] {
  return fetchMock.mock.calls.map(([input]) => {
    if (typeof input === "string") return input;
    if (input instanceof URL) return input.toString();
    return input.url;
  });
}

describe("webApi", () => {
  const fetchMock = vi.fn<typeof fetch>();

  beforeEach(() => {
    fetchMock.mockReset();
    fetchMock.mockImplementation(() => Promise.resolve(jsonResponse({ api_version: 1 })));
    vi.stubGlobal("fetch", fetchMock);
    sessionStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("exports only the nine canonical namespaces", () => {
    expect(Object.keys(webApi).sort()).toEqual([
      "catalog",
      "context",
      "credentials",
      "init",
      "platform",
      "review",
      "settings",
      "tasks",
      "workspaces",
    ]);
    expect("runs" in webApi).toBe(false);
  });

  it("maps platform, init, workspace, catalog, and settings operations", async () => {
    await webApi.platform.status();
    await webApi.init.status();
    await webApi.init.providers({ providers: [], tiers: [], expected_revision: revision });
    await webApi.init.defaultTier({ tier_id: "tier:balanced", expected_revision: revision });
    await webApi.init.workspace({
      workspace: {
        root_id: "root_000000000000000000000000",
        relative_path: "project",
        label: "Project",
        expected_revision: revision,
      },
    });
    await webApi.init.test({ tier_id: "tier:balanced", expected_revision: revision });
    await webApi.init.complete({ expected_revision: revision });
    await webApi.workspaces.registrationRoots();
    await webApi.workspaces.list();
    await webApi.workspaces.register({
      root_id: "root_000000000000000000000000",
      relative_path: "project",
      label: "Project",
      expected_revision: revision,
    });
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await webApi.workspaces.remove(workspaceId, { expected_revision: revision });
    await webApi.catalog.platform();
    await webApi.catalog.workspace(workspaceId, { search: null });
    await webApi.settings.get();
    await webApi.settings.update({
      expected_revision: revision,
      patch: {
        default_tier: null,
        default_workspace_id: null,
        max_concurrent_runs: 16,
      },
    });

    expect(requestedPaths(fetchMock)).toEqual([
      "/api/v1/status",
      "/api/v1/init/status",
      "/api/v1/init/providers",
      "/api/v1/init/default-tier",
      "/api/v1/init/workspace",
      "/api/v1/init/test",
      "/api/v1/init/complete",
      "/api/v1/registration-roots",
      "/api/v1/workspaces",
      "/api/v1/workspaces",
      `/api/v1/workspaces/${workspaceId}`,
      "/api/v1/catalog",
      `/api/v1/workspaces/${workspaceId}/catalog`,
      "/api/v1/settings",
      "/api/v1/settings",
    ]);
  });

  it("maps every task and context operation without a top-level runs client", async () => {
    await webApi.tasks.list({ workspace_id: workspaceId, search: null, cursor: null, limit: 100 });
    await webApi.tasks.create({ workspace_id: workspaceId, idempotency_key: "create-1" });
    await webApi.tasks.get(taskId);
    await webApi.tasks.timeline(taskId, { before: "opaque timeline cursor", limit: 500 });
    await webApi.tasks.submitRun(taskId, {
      expected_task_revision: 2,
      idempotency_key: "submit-1",
      message: "Inspect the project",
      tier_id: "tier:balanced",
      skill_ids: [],
    });
    await webApi.tasks.stopRun(taskId, {
      expected_task_revision: 3,
      idempotency_key: "stop-1",
    });
    await webApi.tasks.respond(taskId, "int_000000000000000000000001", {
      expected_task_revision: 3,
      idempotency_key: "respond-1",
      choice_id: "continue",
    });
    await webApi.tasks.subscribe(taskId, 8, new AbortController().signal);
    await webApi.context.current(taskId);
    await webApi.context.historical(taskId, "req_000000000000000000000001");
    await webApi.context.agentThread(taskId, "conv_000000000000000000000001");

    expect(requestedPaths(fetchMock)).toEqual([
      `/api/v1/tasks?workspace_id=${workspaceId}&limit=100`,
      "/api/v1/tasks",
      `/api/v1/tasks/${taskId}`,
      `/api/v1/tasks/${taskId}/timeline?before=opaque+timeline+cursor&limit=500`,
      `/api/v1/tasks/${taskId}/runs`,
      `/api/v1/tasks/${taskId}/stop`,
      `/api/v1/tasks/${taskId}/interactions/int_000000000000000000000001`,
      `/api/v1/tasks/${taskId}/stream?after=8`,
      `/api/v1/tasks/${taskId}/context`,
      `/api/v1/tasks/${taskId}/context/req_000000000000000000000001`,
      `/api/v1/tasks/${taskId}/agents/conv_000000000000000000000001`,
    ]);
  });

  it("maps every review operation", async () => {
    await webApi.review.tree(workspaceId, { prefix: "src", cursor: null, limit: 100 });
    await webApi.review.search(workspaceId, {
      query: "client",
      prefix: "src",
      cursor: null,
      limit: 100,
    });
    await webApi.review.file(workspaceId, { path: "src/lib.rs", start_line: 1, end_line: 80 });
    await webApi.review.changes(workspaceId, { cursor: null, limit: 100 });
    await webApi.review.diff(workspaceId, {
      path: "src/lib.rs",
      revision,
      cursor: null,
      limit: 100,
    });
    await webApi.review.submissions(taskId, { cursor: null, limit: 100 });
    await webApi.review.submit(taskId, {
      expected_task_revision: 2,
      idempotency_key: "review-1",
      notes: [],
    });

    expect(requestedPaths(fetchMock)).toEqual([
      `/api/v1/workspaces/${workspaceId}/files/tree?prefix=src&limit=100`,
      `/api/v1/workspaces/${workspaceId}/files/search?query=client&prefix=src&limit=100`,
      `/api/v1/workspaces/${workspaceId}/files/content?path=src%2Flib.rs&start_line=1&end_line=80`,
      `/api/v1/workspaces/${workspaceId}/changes?limit=100`,
      `/api/v1/workspaces/${workspaceId}/changes/diff?path=src%2Flib.rs&revision=${revision}&limit=100`,
      `/api/v1/tasks/${taskId}/review/submissions?limit=100`,
      `/api/v1/tasks/${taskId}/review/annotations`,
    ]);
  });

  it("keeps opaque timeline cursors transparent but rejects numeric before values", async () => {
    const timeline = webApi.tasks.timeline as unknown as (
      id: string,
      query: { before: number; limit: number },
    ) => Promise<unknown>;

    await expect(timeline(taskId, { before: 8, limit: 500 })).rejects.toThrow(
      "timeline before must be an opaque PageCursor string or null",
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("requests the task subscription as NDJSON without a request body", async () => {
    await webApi.tasks.subscribe(taskId, null, new AbortController().signal);

    const [, request] = fetchMock.mock.calls[0] ?? [];
    expect(request?.method).toBe("GET");
    expect(request?.body).toBeUndefined();
    expect(new Headers(request?.headers).get("Accept")).toBe("application/x-ndjson");
  });

  it("rejects a task subscription that has no response body", async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 200 }));

    await expect(
      webApi.tasks.subscribe(taskId, null, new AbortController().signal),
    ).rejects.toThrow("task subscription response has no body");
  });

  it("stores bearer credentials outside response data and adds authorization", async () => {
    webApi.credentials.set("fixture-token");
    expect(webApi.credentials.current()).toBe("fixture-token");

    await webApi.platform.status();

    const request = fetchMock.mock.calls[0]?.[1];
    expect(new Headers(request?.headers).get("Authorization")).toBe("Bearer fixture-token");
    webApi.credentials.clear();
    expect(webApi.credentials.current()).toBeNull();
  });

  it("throws the typed API error returned by a failed request", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(
        {
          api_version: 1,
          code: "task_busy",
          message: "Task already has an active run",
          trace_id: "trace_fixture",
          details: null,
        },
        409,
      ),
    );

    try {
      await webApi.tasks.get(taskId);
      expect.unreachable("request should reject");
    } catch (error) {
      expect(error).toBeInstanceOf(WebApiError);
      expect(error).toMatchObject({
        name: "WebApiError",
        status: 409,
        code: "task_busy",
        traceId: "trace_fixture",
      });
    }
  });

  it("decodes and rejects malformed non-2xx API errors", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(
        {
          api_version: 1,
          code: "unknown_code",
          message: "Invalid error fixture",
          trace_id: "trace_fixture",
          details: null,
        },
        400,
      ),
    );
    await expect(webApi.tasks.get(taskId)).rejects.toBeInstanceOf(ContractDecodeError);

    fetchMock.mockResolvedValueOnce(
      jsonResponse(
        {
          api_version: 1,
          code: "task_busy",
          message: "Missing details fixture",
          trace_id: "trace_fixture",
        },
        409,
      ),
    );
    await expect(webApi.tasks.get(taskId)).rejects.toBeInstanceOf(ContractDecodeError);
  });
});
