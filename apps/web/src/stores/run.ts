import { create } from "zustand";
import type { Turn, ToolRender, PermissionRender } from "@/types/turn";
import { parseWireEvent } from "@/adapters/stream";
import type { TurnMutation } from "@/adapters/stream";
import { sendResponse } from "@/api/responses";

interface RunState {
  turns: Turn[];
  status: "idle" | "loading" | "streaming" | "done" | "error";
  error?: string;
  pendingPermission: PermissionRender | null;
  loadTurns: (turns: Turn[]) => void;
  clear: () => void;
  pushWireLine: (line: string) => void;
  pushActiveStream: (lines: Array<Record<string, unknown>>) => void;
  setStatus: (status: RunState["status"]) => void;
  respondToPermission: (runId: string, interactionId: string, choice: string) => Promise<void>;
}

export const useRunStore = create<RunState>()((set) => ({
  turns: [],
  status: "idle",
  pendingPermission: null,

  loadTurns: (turns) => {
    set({ turns, status: "done" });
  },

  clear: () => {
    set({ turns: [], status: "idle", error: undefined, pendingPermission: null });
  },

  pushWireLine: (line) => {
    const mutations = parseWireEvent(line);
    set((state) => {
      let { turns } = state;
      let pendingPermission = state.pendingPermission;
      for (const m of mutations) {
        turns = applyMutation(turns, m);
        if (m.type === "add_permission") pendingPermission = m.permission;
        if (m.type === "finish_turn" || m.type === "new_turn") pendingPermission = null;
      }
      return { turns, status: "streaming", pendingPermission };
    });
  },

  pushActiveStream: (lines) => {
    set((state) => {
      let { turns } = state;
      let lastMutationType: string | null = null;
      let lastPermission: PermissionRender | null = null;
      for (const line of lines) {
        const wire = JSON.stringify(line);
        for (const m of parseWireEvent(wire)) {
          turns = applyMutation(turns, m);
          lastMutationType = m.type;
          if (m.type === "add_permission") lastPermission = m.permission;
          if (m.type === "finish_turn" || m.type === "new_turn") {
            lastPermission = null;
          }
        }
      }
      const pendingPermission = lastMutationType === "add_permission" ? lastPermission : null;
      return { turns, status: "streaming", pendingPermission };
    });
  },

  setStatus: (status) => {
    set({ status });
  },

  respondToPermission: async (runId, interactionId, choice) => {
    set({ pendingPermission: null });
    await sendResponse(runId, interactionId, choice);
  },
}));

function applyMutation(turns: Turn[], m: TurnMutation): Turn[] {
  const copy = [...turns];
  const lastRaw = copy[copy.length - 1];
  if (!lastRaw && m.type !== "new_turn") return copy;
  const last = lastRaw
    ? {
        ...lastRaw,
        agent: {
          ...lastRaw.agent,
          tools: [...lastRaw.agent.tools],
          permissions: [...lastRaw.agent.permissions],
        },
      }
    : null;

  switch (m.type) {
    case "new_turn":
      copy.push(m.turn);
      break;
    case "append_text":
      if (last) last.agent.text = (last.agent.text ?? "") + m.text;
      if (last) copy[copy.length - 1] = last;
      break;
    case "append_thinking":
      if (last) last.agent.thinking = (last.agent.thinking ?? "") + m.text;
      if (last) copy[copy.length - 1] = last;
      break;
    case "upsert_tool": {
      if (!last) break;
      const idx = last.agent.tools.findIndex((t) => t.id === m.tool.id);
      if (idx >= 0) {
        const existing = last.agent.tools[idx] as ToolRender;
        last.agent.tools[idx] = {
          ...existing,
          ...m.tool,
          name: m.tool.name || existing.name,
          kind:
            m.tool.kind !== "simple" || m.tool.name
              ? m.tool.kind
              : existing.kind,
          subEvents:
            m.tool.subEvents.length > 0
              ? m.tool.subEvents
              : existing.subEvents,
        };
      } else {
        last.agent.tools.push(m.tool);
      }
      copy[copy.length - 1] = last;
      break;
    }
    case "append_sub_event": {
      if (!last) break;
      const t = last.agent.tools.find((t) => t.id === m.toolId);
      if (t) t.subEvents.push(m.event);
      copy[copy.length - 1] = last;
      break;
    }
    case "add_permission":
      if (last) last.agent.permissions.push(m.permission);
      if (last) copy[copy.length - 1] = last;
      break;
    case "set_model":
      if (last) last.agent.model = { provider: m.provider, model: m.model };
      if (last) copy[copy.length - 1] = last;
      break;
    case "set_user_text":
      if (last) last.userText = m.text;
      if (last) copy[copy.length - 1] = last;
      break;
    case "finish_turn":
      if (last) last.status = m.status;
      if (last) copy[copy.length - 1] = last;
      break;
    case "error":
      if (last) last.agent.error = { code: m.code, message: m.message };
      if (last) copy[copy.length - 1] = last;
      break;
  }
  return copy;
}
