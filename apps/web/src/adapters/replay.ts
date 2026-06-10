import type { Turn, AgentMessage, ToolRender } from "@/types/turn";

export interface EventPayload {
  kind: string;
  [key: string]: unknown;
}

export function replayToTurns(events: EventPayload[]): Turn[] {
  const visibleEvents = filterRolledBackEvents(events);
  const byTurn = new Map<number, EventPayload[]>();
  for (const e of visibleEvents) {
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
    switch (e.kind) {
      case "message.user":
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
      case "message.assistant":
        agent.text = e.text as string;
        break;
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
          code: e.error_kind as string,
          message: e.message as string,
        };
        break;
      case "permission.requested":
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
    status: turnStatus(events),
  };
}

function filterRolledBackEvents(events: EventPayload[]): EventPayload[] {
  const rollbacks = activeRollbacks(events);
  if (rollbacks.size === 0) return events;
  const turnConversations = buildTurnConversations(events);

  return events.filter((event) => {
    const turn = event.turn as number | undefined;
    const conversation = eventConversation(event, turnConversations);
    if (!conversation) return true;

    const rollback = rollbacks.get(conversation);
    if (!rollback) return true;

    if (turn !== undefined) return turn < rollback.toTurn;

    const id = event.id as number | undefined;
    return id === undefined || id <= rollback.toEventId;
  });
}

function activeRollbacks(events: EventPayload[]): Map<string, { toTurn: number; toEventId: number }> {
  const undone = new Set<number>();
  for (const event of events) {
    if (event.kind === "conversation.rollback.undone") {
      const rollbackId = event.rollback_event_id as number | undefined;
      if (rollbackId !== undefined) undone.add(rollbackId);
    }
  }

  const rollbacks = new Map<string, { toTurn: number; toEventId: number }>();
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (event?.kind !== "conversation.rollback") continue;

    const id = event.id as number | undefined;
    const conversation = event.conversation as string | undefined;
    const toTurn = event.to_turn as number | undefined;
    const toEventId = event.to_event_id as number | undefined;
    const scope = event.scope as string | undefined;
    if (!conversation || toTurn === undefined || toEventId === undefined || scope === "file_changes") continue;
    if (id !== undefined && undone.has(id)) continue;
    if (!rollbacks.has(conversation)) rollbacks.set(conversation, { toTurn, toEventId });
  }
  return rollbacks;
}

function buildTurnConversations(events: EventPayload[]): Map<number, string> {
  const conversations = new Map<number, string>();
  for (const event of events) {
    const turn = event.turn as number | undefined;
    const conversation = event.conversation as string | undefined;
    if (turn !== undefined && conversation && !conversations.has(turn)) {
      conversations.set(turn, conversation);
    }
  }
  return conversations;
}

function eventConversation(
  event: EventPayload,
  turnConversations: Map<number, string>,
): string | undefined {
  const conversation = event.conversation as string | undefined;
  if (conversation) return conversation;

  const turn = event.turn as number | undefined;
  if (turn !== undefined) return turnConversations.get(turn) ?? "main";

  return undefined;
}

function turnStatus(events: EventPayload[]): Turn["status"] {
  if (events.some((e) => e.kind === "turn.completed")) return "complete";
  if (events.some((e) => e.kind === "turn.cancelled")) return "cancelled";
  return "error";
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
