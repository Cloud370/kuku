import type { Turn, SubEvent, ToolRender, PermissionRender } from "@/types/turn";

interface WireEvent {
  type: string;
  [key: string]: unknown;
}

export type TurnMutation =
  | { type: "new_turn"; turn: Turn }
  | { type: "append_text"; text: string }
  | { type: "append_thinking"; text: string }
  | { type: "upsert_tool"; tool: ToolRender }
  | { type: "append_sub_event"; toolId: string; event: SubEvent }
  | { type: "set_model"; provider: string; model: string }
  | { type: "set_user_text"; text: string }
  | { type: "add_permission"; permission: PermissionRender }
  | { type: "finish_turn"; status: Turn["status"] }
  | { type: "error"; code: string; message: string };

export function parseWireEvent(line: string): TurnMutation[] {
  let e: WireEvent;
  try {
    e = JSON.parse(line) as WireEvent;
  } catch {
    return [];
  }

  switch (e.type) {
    case "run_start":
      return [];
    case "turn_start":
      return [
        {
          type: "new_turn",
          turn: {
            turnNumber: e.turn as number,
            userText: "",
            agent: { tools: [], permissions: [] },
            status: "streaming",
          },
        },
      ];
    case "text":
      return [{ type: "append_text", text: e.content as string }];
    case "thinking":
      return [{ type: "append_thinking", text: e.content as string }];
    case "tool_start":
      return [
        {
          type: "upsert_tool",
          tool: {
            id: e.id as string,
            name: e.tool as string,
            summary: e.summary as string,
            kind: mapKind(e.kind),
            status: "running",
            subEvents: [],
          },
        },
      ];
    case "tool_output": {
      const subEvent = mapSubEvent(e.event as Record<string, unknown>);
      return subEvent
        ? [{ type: "append_sub_event", toolId: e.id as string, event: subEvent }]
        : [];
    }
    case "tool_end":
      return [
        {
          type: "upsert_tool",
          tool: {
            id: e.id as string,
            name: "",
            summary: (e.summary as string | undefined) ?? "",
            kind: "simple",
            status: e.status === "ok" ? "completed" : "error",
            modelContent: (e.model_content as string | undefined) ?? "",
            result: e.result,
            subEvents: [],
          },
        },
      ];
    case "done":
      return [{ type: "finish_turn", status: "complete" }];
    case "error":
      return [
        {
          type: "error",
          code: (e.code as string | undefined) ?? "unknown",
          message: (e.message as string | undefined) ?? "unknown error",
        },
      ];
    case "cancelled":
      return [{ type: "finish_turn", status: "cancelled" }];
    case "permission": {
      const perm: PermissionRender = {
        id: e.id as string,
        tool: e.tool as string,
        risk: e.risk as string,
        summary: e.summary as string,
      };
      return [{ type: "add_permission", permission: perm }];
    }
    case "model_request":
      return [
        {
          type: "set_model",
          provider: e.provider as string,
          model: e.model as string,
        },
      ];
    default:
      return [];
  }
}

function mapKind(k: unknown): ToolRender["kind"] {
  if (typeof k === "object" && k !== null) {
    if ("agent" in (k as Record<string, unknown>)) return "agent";
    if ("command" in (k as Record<string, unknown>)) return "command";
  }
  return "simple";
}

function mapSubEvent(e: Record<string, unknown>): SubEvent | null {
  if (typeof e.text === "string") return { type: "text", content: e.text };
  if (typeof e.thinking === "string")
    return { type: "thinking", content: e.thinking };
  if (typeof e.stdout === "string") return { type: "stdout", text: e.stdout };
  if (typeof e.stderr === "string") return { type: "stderr", text: e.stderr };
  if (e.tool_start) {
    const ts = e.tool_start as Record<string, unknown>;
    return {
      type: "tool_start",
      id: ts.id as string,
      tool: ts.tool as string,
      summary: ts.summary as string,
    };
  }
  if (e.tool_end) {
    const te = e.tool_end as Record<string, unknown>;
    return {
      type: "tool_end",
      id: te.id as string,
      status: te.status as string,
      summary: te.summary as string,
    };
  }
  return null;
}
