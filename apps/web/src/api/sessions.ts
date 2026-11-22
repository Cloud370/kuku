import { ApiError, get, post, request } from "./client";

export type Target = {
  workspace: string;
  session: string;
  agent: string;
};

export type AgentStatus = "idle" | "running" | "waiting" | "stopped" | "interrupted";

export interface SessionSummary {
  session_id: string;
  workspace: string;
  title: string;
  status: string;
  mtime: string;
  waiting_agents?: string[];
}

export interface AgentConfig {
  connection?: string | null;
  tier?: string | null;
  model?: string | null;
  reasoning_effort?: string | null;
  service_tier?: string | null;
  workspace?: string | null;
  output?: RequestedOutput | null;
}

export type SendTiming = "steer" | "next_turn";

export type Command =
  | { type: "send"; text: string; timing: SendTiming; create?: AgentConfig }
  | { type: "create_agent" | "configure"; config: AgentConfig }
  | { type: "reply_question"; request_id: string; text: string }
  | { type: "reply_permission"; request_id: string; allow: boolean }
  | { type: "withdraw"; input_command_id: string }
  | { type: "advance" }
  | { type: "cancel"; execution_id: number };

export type Intent = Target & { commandId: string; command: Command };

export interface Receipt {
  command_id: string;
  event_id: number;
  agent: string;
}

export interface CommandResult {
  ok: true;
  receipt: Receipt;
  replayed?: boolean;
  status?: "queued" | "accepted" | "withdrawn";
}

export interface PermissionRequest {
  id: string;
  tool: string;
  risk: string;
  summary: string;
  candidate?: string;
}

export interface QuestionRequest {
  id: string;
  text: string;
}

export interface AgentSummary {
  address: string;
  status: AgentStatus;
  turn: number | null;
  execution_id: number | null;
  pending_inputs: number;
}

type RequestedOutput = "text" | "image";
type CapabilitySource = "model_list" | "protocol_model_list" | "manual" | "protocol_manual" | "adapter_profile";

interface Declaration<T> {
  value: T;
  source: CapabilitySource;
}

interface ModelCapabilities {
  reasoning_efforts: string[];
  reasoning_source?: CapabilitySource | null;
  reasoning_default?: Declaration<string> | null;
  input_modalities?: Declaration<string[]> | null;
  output_modalities?: Declaration<string[]> | null;
  interfaces?: Declaration<string[]> | null;
  system_prompt_update?: Declaration<string> | null;
  media_adapter: "text_only" | "responses_image_tool_results";
}

interface ModelSelection {
  model: string;
  connection: string;
  request_url?: string;
  settings: { reasoning_effort?: string | null };
  capabilities?: ModelCapabilities;
  output?: RequestedOutput;
}

export interface Snapshot {
  ok: true;
  workspace: string;
  session_id: string;
  cursor: number;
  cleaning_up?: boolean;
  agents: AgentSummary[];
  agent: {
    address: string;
    status: AgentStatus;
    config: AgentConfig;
    turn: number | null;
    last_error: string | null;
    permission: PermissionRequest | null;
    question: QuestionRequest | null;
    usage: { input_tokens: number | null; output_tokens: number | null };
  };
  pending: {
    receipt: Receipt;
    input: { text: string; timing?: "next_step" | "next_turn" };
  }[];
  selection: ModelSelection | null;
  selection_error?: { code: string; message: string } | null;
  pending_selection?: ModelSelection | null;
  pending_selection_error?: { code: string; message: string } | null;
}

type EntryBase = { id: number; agent: string; turn?: number | null };
export type ToolCall = {
  type: "tool_call";
  tool: string;
  tool_call_id: string;
  args?: unknown;
};
export type ToolResult = {
  type: "tool_result";
  tool_call_id: string;
  tool?: string;
  status: string;
  summary: string;
  structured?: Record<string, unknown> | null;
  model_content?: string | null;
  output_id?: string | null;
};
export type Entry = EntryBase &
  (
    | { type: "user"; text: string; command_id?: string }
    | { type: "assistant"; text?: string | null; thinking?: string | null; request_id?: string }
    | ToolCall
    | ToolResult
    | { type: "question" | "permission"; text?: string | null }
    | { type: "question_reply"; request_id: string; text: string }
    | { type: "permission_reply"; request_id: string; allow: boolean }
    | { type: "status"; status: "cancelled" | "interrupted"; text?: string | null }
    | { type: "error"; text?: string | null; code?: string; message?: string }
  );

export interface HistoryPage {
  ok: true;
  entries: Entry[];
  next_before: number | null;
}

export interface ModelTier {
  name: string;
  connection: string;
  model: string;
  error?: { code: string; message: string };
  purpose?: string | null;
  request_url?: string | null;
  settings?: ModelSelection["settings"] | null;
  capabilities?: ModelCapabilities | null;
  output?: RequestedOutput | null;
}

export type LiveEvent =
  | { type: "text" | "thinking"; content: string }
  | { type: "model_request"; request_id: string; turn: number }
  | { type: "tool_start"; id: string; tool: string; summary: string }
  | { type: "tool_output"; id: string; event: { stdout?: string; stderr?: string } }
  | { type: "tool_end"; id: string }
  | { type: "done" | "cancelled" };

export type Observation =
  | {
      type: "committed";
      id: number;
      agent?: string;
      kind: string;
      entry?: Entry;
      state_changed?: boolean;
    }
  | {
      type: "live";
      agent: string;
      execution_id: number;
      cursor: number;
      turn: number | null;
      event: LiveEvent;
    }
  | { type: "lagged"; skipped: number }
  | { type: "heartbeat" };

export interface OutputSegment {
  stream: "stdout" | "stderr";
  text: string;
  offset: number;
  bytes: number;
}

export interface OutputChunk {
  offset: number;
  next_offset: number;
  total_bytes: number;
  segments: OutputSegment[];
  has_more: boolean;
  capture_truncated: boolean;
  cleanup_incomplete: boolean;
  lossy_utf8: boolean;
}

const sessionPath = (session: string) => `/sessions/${encodeURIComponent(session)}`;
const targetQuery = ({ workspace, agent }: Target) => new URLSearchParams({ workspace, agent });

export function fetchSessions(workspace: string, search: string, signal?: AbortSignal) {
  const params = new URLSearchParams({ workspace, search });
  return get<{ ok: true; sessions: SessionSummary[] }>(`/sessions?${params}`, signal);
}

export function fetchWorkspaces(signal?: AbortSignal) {
  return get<{ ok: true; workspaces: string[] }>("/workspaces", signal);
}

export function fetchModels(signal?: AbortSignal) {
  return get<{ ok: true; default_tier: string | null; tiers: ModelTier[] }>("/models", signal);
}

export function resolveWorkspace(workspace: string, signal?: AbortSignal) {
  const params = new URLSearchParams({ workspace });
  return get<{ ok: true; workspace: string }>(`/workspaces/resolve?${params}`, signal);
}

export function fetchSnapshot(target: Target, signal: AbortSignal) {
  return get<Snapshot>(`${sessionPath(target.session)}/snapshot?${targetQuery(target)}`, signal);
}

export function fetchAgents({ workspace, session }: Target, signal: AbortSignal) {
  const params = new URLSearchParams({ workspace });
  return get<{ ok: true; agents: AgentSummary[] }>(
    `${sessionPath(session)}/agents?${params}`,
    signal,
  );
}

export function fetchHistory(target: Target, before: number, signal: AbortSignal) {
  const params = targetQuery(target);
  params.set("before", String(before));
  params.set("limit", "60");
  return get<HistoryPage>(`${sessionPath(target.session)}/history?${params}`, signal);
}

export async function sendCommand(
  { workspace, session, agent, commandId, command }: Intent,
  replayOnly = false,
) {
  const path = `${sessionPath(session)}/commands${replayOnly ? "?replay_only=true" : ""}`;
  const result = await post<Partial<CommandResult> | null>(path, {
    workspace,
    agent,
    command_id: commandId,
    ...command,
  });
  if (
    !result?.ok ||
    result.receipt?.command_id !== commandId ||
    result.receipt.agent !== agent ||
    !Number.isSafeInteger(result.receipt.event_id)
  ) {
    throw new Error("The server returned an invalid command receipt.");
  }
  return { ...result, ok: true, receipt: result.receipt } satisfies CommandResult;
}

export async function findReceipt({ workspace, session, commandId }: Intent) {
  const params = new URLSearchParams({ workspace });
  const path = `${sessionPath(session)}/receipts/${encodeURIComponent(commandId)}?${params}`;
  try {
    const result = await get<Partial<CommandResult> | null>(path);
    if (
      !result?.ok ||
      result.receipt?.command_id !== commandId ||
      !Number.isSafeInteger(result.receipt.event_id)
    ) {
      throw new Error("The server returned an invalid command receipt.");
    }
    return { ...result, ok: true, receipt: result.receipt } satisfies CommandResult;
  } catch (error) {
    if (error instanceof ApiError && error.status === 404 && error.code === "receipt_not_found") {
      return null;
    }
    throw error;
  }
}

export async function fetchOutput(
  target: Target,
  id: string,
  offset: number,
  signal?: AbortSignal,
) {
  const params = new URLSearchParams({
    workspace: target.workspace,
    agent: target.agent,
    offset: String(offset),
    max_bytes: String(16 * 1024),
  });
  const path = `${sessionPath(target.session)}/outputs/${encodeURIComponent(id)}?${params}`;
  const response = await get<{ ok: true; output: OutputChunk }>(path, signal);
  return response.output;
}

export async function* observe(
  target: Target,
  after: number,
  signal: AbortSignal,
  onConnected?: () => void,
): AsyncGenerator<Observation> {
  const params = targetQuery(target);
  params.set("after", String(after));
  const response = await request(`${sessionPath(target.session)}/observe?${params}`, { signal });
  if (!response.body) throw new Error("The server returned an empty observation stream.");
  onConnected?.();
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      buffer += decoder.decode(value, { stream: !done });
      const lines = buffer.split("\n");
      buffer = lines.pop() ?? "";
      for (const line of lines) {
        if (line.trim()) yield decodeObservation(line);
      }
      if (done) {
        if (buffer.trim()) yield decodeObservation(buffer);
        return;
      }
    }
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}

function decodeObservation(line: string): Observation {
  const frame = JSON.parse(line) as
    | Partial<Observation>
    | { type: "error"; code: string; message: string }
    | null;
  if (frame?.type === "error") throw new ApiError(500, frame.code, frame.message);
  if (
    frame?.type === "committed" &&
    Number.isSafeInteger(frame.id) &&
    typeof frame.kind === "string"
  )
    return frame as Observation;
  if (
    frame?.type === "live" &&
    typeof frame.agent === "string" &&
    Number.isSafeInteger(frame.execution_id) &&
    Number.isSafeInteger(frame.cursor) &&
    typeof frame.event?.type === "string"
  ) {
    return frame as Observation;
  }
  if (frame?.type === "heartbeat") return { type: frame.type };
  if (frame?.type === "lagged" && Number.isSafeInteger(frame.skipped)) return frame as Observation;
  throw new Error("The server returned an invalid observation frame.");
}
