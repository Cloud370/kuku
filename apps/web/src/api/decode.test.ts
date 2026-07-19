import { describe, expect, it } from "vitest";

import { ContractDecodeError, decodeApiError, decodeTaskStreamEvent } from "./decode";

function validStreamEvent(): Record<string, unknown> {
  return {
    api_version: 1,
    cursor: 8,
    task_revision: 2,
    task_id: "tsk_000000000000000000000001",
    event: {
      type: "changes_applied",
      changes: [],
      timeline_window: null,
    },
  };
}

function validApiError(): Record<string, unknown> {
  return {
    api_version: 1,
    code: "task_busy",
    message: "Task already has an active run",
    trace_id: "trace_fixture",
    details: null,
  };
}

describe("decodeTaskStreamEvent", () => {
  it("returns a schema-valid stream event", () => {
    const value = validStreamEvent();

    expect(decodeTaskStreamEvent(value)).toEqual(value);
  });

  it("rejects an unsupported API version", () => {
    const value = validStreamEvent();
    value.api_version = 2;

    expect(() => decodeTaskStreamEvent(value)).toThrow(ContractDecodeError);
  });

  it("rejects an unknown outer delta kind", () => {
    const value = validStreamEvent();
    value.event = { type: "record_replayed" };

    expect(() => decodeTaskStreamEvent(value)).toThrow(ContractDecodeError);
  });

  it("rejects an unknown inner change kind", () => {
    const value = validStreamEvent();
    value.event = {
      type: "changes_applied",
      changes: [{ type: "message_replaced" }],
      timeline_window: null,
    };

    expect(() => decodeTaskStreamEvent(value)).toThrow(ContractDecodeError);
  });

  it("rejects a missing required nullable timeline window", () => {
    const value = validStreamEvent();
    value.event = { type: "changes_applied", changes: [] };

    expect(() => decodeTaskStreamEvent(value)).toThrow(ContractDecodeError);
  });
});

describe("decodeApiError", () => {
  it("returns a schema-valid typed API error", () => {
    const value = validApiError();

    expect(decodeApiError(value)).toEqual(value);
  });

  it("rejects an unknown API error code", () => {
    const value = validApiError();
    value.code = "unknown_code";

    expect(() => decodeApiError(value)).toThrow(ContractDecodeError);
  });

  it("rejects a missing required nullable details key", () => {
    const value = validApiError();
    delete value.details;

    expect(() => decodeApiError(value)).toThrow(ContractDecodeError);
  });
});
