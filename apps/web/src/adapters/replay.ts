import type { Turn, AgentMessage, ToolRender } from "@/types/turn";

export interface EventPayload {
  type: string;
  [key: string]: unknown;
}

export function replayToTurns(events: EventPayload[]): Turn[] {
  const byTurn = new Map<number, EventPayload[]>();
  for (const e of events) {
    const t = e.turn as number | undefined;
    if (t) {
      const list = byTurn.get(t) ?? [];
      list.push(e);
      byTurn.set(t, list);
    }
  }

  const turns: Turn[] = [];
  for (const turnNum of [...byTurn.keys()].sort((a, b) => a - b)) {
    const events = byTurn.get(turnNum);
    if (events) turns.push(buildTurn(turnNum, events));
  }
  return turns;
}

function buildTurn(turnNumber: number, events: EventPayload[]): Turn {
  let userText = "";
  const agent: AgentMessage = { tools: [], permissions: [] };
  const toolCallMap = new Map<string, { name: string }>();

  for (const e of events) {
    switch (e.type) {
      case "user.input":
        userText = e.text as string;
        break;
      case "model.request":
        agent.model = {
          provider: e.provider as string,
          model: e.model as string,
        };
        break;
      case "model.response": {
        const text = e.text as string | undefined;
        const thinking = e.thinking as string | undefined;
        if (text) agent.text = text;
        if (thinking) agent.thinking = thinking;
        break;
      }
      case "tool.call":
        toolCallMap.set(e.tool_call_id as string, {
          name: e.tool as string,
        });
        break;
      case "tool.result": {
        const tc = toolCallMap.get(e.tool_call_id as string);
        agent.tools.push({
          id: e.tool_call_id as string,
          name: tc?.name ?? "unknown",
          summary: (e.summary as string | undefined) ?? "",
          kind: inferKind(tc?.name ?? ""),
          status: mapToolStatus(e.status as string),
          modelContent: e.model_content as string | undefined,
          result: e.structured,
          subEvents: [],
        });
        break;
      }
      case "model.error":
        agent.error = {
          code: e.kind as string,
          message: e.message as string,
        };
        break;
      case "permission.request":
        agent.permissions.push({
          id: e.tool_call_id as string,
          tool: e.tool as string,
          risk: e.risk as string,
          summary: e.summary as string,
        });
        break;
    }
  }

  return {
    turnNumber,
    userText,
    agent,
    status: events.some((e) => e.type === "turn.end") ? "complete" : "error",
  };
}

function inferKind(name: string): ToolRender["kind"] {
  if (name === "agent") return "agent";
  if (name === "run_command") return "command";
  return "simple";
}

function mapToolStatus(s: string): ToolRender["status"] {
  if (s === "ok") return "completed";
  if (s === "error" || s === "blocked") return s;
  return "completed";
}
