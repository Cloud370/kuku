import { describe, expect, it } from "vitest";
import { replayToTurns, type EventPayload } from "./replay";

describe("replayToTurns", () => {
  it("hides conversation turns after an active rollback", () => {
    const turns = replayToTurns([
      { kind: "turn.started", conversation: "main", turn: 1 },
      { kind: "message.user", conversation: "main", turn: 1, text: "keep" },
      { kind: "model.response", turn: 1, text: "kept answer" },
      { kind: "turn.completed", conversation: "main", turn: 1 },
      { kind: "turn.started", conversation: "main", turn: 2 },
      { kind: "message.user", conversation: "main", turn: 2, text: "rolled back" },
      { kind: "model.response", turn: 2, text: "rolled-back answer" },
      { kind: "turn.completed", conversation: "main", turn: 2 },
      {
        kind: "conversation.rollback",
        conversation: "main",
        to_turn: 2,
        to_event_id: 6,
        scope: "messages",
      },
    ] satisfies EventPayload[]);

    expect(turns.map((turn) => turn.userText)).toEqual(["keep"]);
    expect(turns.map((turn) => turn.agent.text)).toEqual(["kept answer"]);
  });

  it("renders canonical permission requests", () => {
    const turns = replayToTurns([
      { kind: "turn.started", conversation: "main", turn: 1 },
      { kind: "message.user", conversation: "main", turn: 1, text: "run tests" },
      {
        kind: "permission.requested",
        turn: 1,
        tool_call_id: "toolu_1",
        tool: "run_command",
        risk: "command",
        summary: "cargo test",
      },
      { kind: "turn.completed", conversation: "main", turn: 1 },
    ] satisfies EventPayload[]);

    expect(turns[0]?.agent.permissions).toEqual([
      {
        id: "toolu_1",
        tool: "run_command",
        risk: "command",
        summary: "cargo test",
      },
    ]);
  });

  it("renders assistant messages when replaying conversation-scoped events", () => {
    const turns = replayToTurns([
      { kind: "turn.started", conversation: "review", turn: 1 },
      { kind: "message.user", conversation: "review", turn: 1, text: "review this" },
      {
        kind: "message.assistant",
        conversation: "review",
        turn: 1,
        message_id: "req_1",
        text: "review result",
      },
      { kind: "turn.completed", conversation: "review", turn: 1 },
    ] satisfies EventPayload[]);

    expect(turns[0]?.agent.text).toBe("review result");
  });

  it("marks cancelled turns as cancelled", () => {
    const turns = replayToTurns([
      { kind: "turn.started", conversation: "main", turn: 1 },
      { kind: "message.user", conversation: "main", turn: 1, text: "stop" },
      { kind: "turn.cancelled", conversation: "main", turn: 1, reason: "user_cancelled" },
    ] satisfies EventPayload[]);

    expect(turns[0]?.status).toBe("cancelled");
  });
});
