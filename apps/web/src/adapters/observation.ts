import type { Entry, Observation, Snapshot } from "@/api/sessions";

export interface ToolPreview {
  id: string;
  name?: string;
  summary?: string;
  lines: { stream: "stdout" | "stderr"; text: string }[];
}

interface Preview {
  executionId: number;
  turn: number | null;
  text: string;
  thinking: string;
  tools: ToolPreview[];
}

export interface ConversationView {
  snapshot: Snapshot | null;
  entries: Entry[];
  before: number | null;
  preview: Preview | null;
  textBarrier: number;
  turnBarrier: number;
  connection: "connecting" | "connected" | "reconnecting" | "failed";
  error?: string;
  errorCode?: string;
  loadingOlder?: boolean;
  historyError?: string;
}

export const emptyConversation: ConversationView = {
  snapshot: null,
  entries: [],
  before: null,
  preview: null,
  textBarrier: 0,
  turnBarrier: 0,
  connection: "connecting",
};

export function mergeEntries(current: Entry[], page: Entry[]) {
  const byId = new Map(current.map((entry) => [entry.id, entry]));
  for (const entry of page) byId.set(entry.id, entry);
  return [...byId.values()].sort((left, right) => left.id - right.id);
}

export function applySnapshot(view: ConversationView, snapshot: Snapshot): ConversationView {
  if (view.snapshot && view.snapshot.cursor > snapshot.cursor) return view;
  const active = snapshot.agents.find(
    ({ address }) => address === snapshot.agent.address,
  )?.execution_id;
  return {
    ...view,
    snapshot,
    preview: view.preview?.executionId === active ? view.preview : null,
  };
}

export function applyObservation(
  view: ConversationView,
  frame: Observation,
  agent: string,
): ConversationView {
  if (frame.type === "heartbeat") return view;
  if (frame.type === "lagged") return { ...view, preview: null };
  if (frame.type === "live") return frame.agent === agent ? applyLive(view, frame) : view;
  if ((frame.agent ?? frame.entry?.agent ?? "main") !== agent) return view;

  const entry = frame.entry?.agent === agent ? frame.entry : undefined;
  const settlesText =
    entry?.type === "assistant" || frame.kind === "model.response" || frame.kind === "model.error";
  const settlesTurn = ["turn.completed", "turn.cancelled", "turn.interrupted"].includes(frame.kind);
  if (!entry && !settlesText && !settlesTurn) return view;
  let preview = view.preview;
  if (settlesTurn) preview = null;
  if (preview && settlesText) preview = { ...preview, text: "", thinking: "" };
  if (preview && entry?.type === "tool_result" && entry.turn === preview.turn) {
    preview = { ...preview, tools: preview.tools.filter(({ id }) => id !== entry.tool_call_id) };
  }
  return {
    ...view,
    entries: entry ? mergeEntries(view.entries, [entry]) : view.entries,
    preview,
    textBarrier:
      settlesText || settlesTurn ? Math.max(view.textBarrier, frame.id) : view.textBarrier,
    turnBarrier: settlesTurn ? Math.max(view.turnBarrier, frame.id) : view.turnBarrier,
  };
}

function applyLive(
  view: ConversationView,
  frame: Extract<Observation, { type: "live" }>,
): ConversationView {
  if (frame.cursor <= view.turnBarrier) return view;
  const active = view.snapshot?.agents.find(({ address }) => address === frame.agent)?.execution_id;
  if (view.snapshot && frame.cursor <= view.snapshot.cursor && active !== frame.execution_id)
    return view;

  let preview = view.preview;
  if (preview?.executionId !== frame.execution_id || preview.turn !== frame.turn) {
    preview = {
      executionId: frame.execution_id,
      turn: frame.turn,
      text: "",
      thinking: "",
      tools: [],
    };
  }
  const event = frame.event;
  if (event.type === "text" || event.type === "thinking") {
    if (frame.cursor <= view.textBarrier) return view;
    const text = (preview[event.type] + event.content).slice(-24_000);
    return { ...view, preview: { ...preview, [event.type]: text } };
  }
  if (event.type !== "tool_start" && event.type !== "tool_output") return view;
  if (
    view.entries.some(
      (entry) =>
        entry.type === "tool_result" &&
        entry.tool_call_id === event.id &&
        entry.turn === frame.turn &&
        entry.id >= frame.cursor,
    )
  )
    return view;

  const existing = preview.tools.find(({ id }) => id === event.id);
  let tool: ToolPreview = existing ?? { id: event.id, lines: [] };
  if (event.type === "tool_start") {
    tool = { ...tool, name: event.tool, summary: event.summary };
  } else {
    const chunks = [
      { stream: "stdout" as const, text: event.event.stdout },
      { stream: "stderr" as const, text: event.event.stderr },
    ];
    tool = {
      ...tool,
      lines: [
        ...tool.lines,
        ...chunks.flatMap(({ stream, text }) =>
          text === undefined ? [] : [{ stream, text: text.slice(-4000) }],
        ),
      ].slice(-50),
    };
  }
  const tools = existing
    ? preview.tools.map((item) => (item.id === tool.id ? tool : item))
    : [...preview.tools, tool].slice(-32);
  return { ...view, preview: { ...preview, tools } };
}
