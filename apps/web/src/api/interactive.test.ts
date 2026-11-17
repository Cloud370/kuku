import { readFileSync } from "node:fs";
import { URL as FileURL } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { observe, sendCommand } from "./sessions";
import type { CommandResult, HistoryPage, Intent, Observation } from "./sessions";
import { applyObservation, emptyConversation } from "@/adapters/observation";

interface ProtocolFixture {
  first_send: Extract<Intent["command"], { type: "send" }> & {
    workspace: string;
    agent: string;
    command_id: string;
  };
  receipt: CommandResult;
  history: HistoryPage;
  observation: Observation[];
  error: { ok: false; code: string; message: string };
}

const fixtureUrl = new FileURL(
  "../../../../crates/kuku-server/tests/common/fixtures/interactive-protocol.json",
  import.meta.url,
);
const fixture = JSON.parse(readFileSync(fixtureUrl, "utf8")) as ProtocolFixture;
const { workspace, agent, command_id: commandId, ...command } = fixture.first_send;
const intent: Intent = { workspace, agent, commandId, session: "fixture", command };
const storedCommands = "kuku-unconfirmed-commands-v1";

afterEach(() => {
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

function failedReceipt() {
  return Response.json(
    { ok: false, code: "receipt_not_found", message: "No receipt" },
    { status: 404 },
  );
}

function postBody(fetchMock: ReturnType<typeof vi.fn>, call: number): unknown {
  const init = fetchMock.mock.calls[call]?.[1] as RequestInit | undefined;
  if (typeof init?.body !== "string") throw new Error("Expected a JSON body");
  return JSON.parse(init.body) as unknown;
}

describe("shared interactive protocol", () => {
  it("posts the canonical first intent and preserves the server receipt and conflict error", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(Response.json(fixture.receipt))
      .mockResolvedValueOnce(Response.json(fixture.error, { status: 409 }));
    vi.stubGlobal("fetch", fetchMock);
    await expect(sendCommand(intent)).resolves.toEqual(fixture.receipt);
    expect(fetchMock.mock.calls[0]?.[0]).toBe("/sessions/fixture/commands");
    expect(postBody(fetchMock, 0)).toEqual(fixture.first_send);
    await expect(sendCommand(intent)).rejects.toMatchObject({
      code: fixture.error.code,
      status: 409,
    });
  });

  it("reads fragmented observations and replaces the transient text with one durable entry", async () => {
    const ndjson = fixture.observation.map((frame) => JSON.stringify(frame)).join("\n");
    const encoded = new TextEncoder().encode(ndjson);
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(encoded.slice(0, 17));
        controller.enqueue(encoded.slice(17, 103));
        controller.enqueue(encoded.slice(103));
        controller.close();
      },
    });
    const mockFetch = vi.fn().mockResolvedValue(new Response(stream));
    vi.stubGlobal("fetch", mockFetch);
    let view = { ...emptyConversation, entries: fixture.history.entries };
    const decoded: Observation[] = [];
    for await (const frame of observe(intent, 18, new AbortController().signal)) {
      decoded.push(frame);
      view = applyObservation(view, frame, "main");
    }
    expect(decoded).toEqual(fixture.observation);
    expect(mockFetch.mock.calls[0]?.[0]).toBe(
      "/sessions/fixture/observe?workspace=%2Fwork%2Fkuku&agent=main&after=18",
    );
    expect(view.entries.map(({ id }) => id)).toEqual([8, 15, 18, 22]);
    expect(view.preview).toBeNull();

    const latePreview = fixture.observation[0];
    if (!latePreview) throw new Error("Missing live example");
    expect(applyObservation(view, latePreview, "main")).toBe(view);
    expect(applyObservation(view, latePreview, "reviewer")).toBe(view);
  });

  it("recovers a lost receipt after a reload using only an identical replay", async () => {
    vi.resetModules();
    const fetchMock = vi
      .fn()
      .mockRejectedValueOnce(new Error("First response lost"))
      .mockRejectedValueOnce(new Error("Browser went offline"));
    vi.stubGlobal("fetch", fetchMock);
    const { useCommands: original } = await import("@/stores/commands");
    await original.getState().submit(intent);
    expect(original.getState().pending).toMatchObject([{ intent, status: "unknown" }]);
    expect(sessionStorage.getItem(storedCommands)).toBe(JSON.stringify([intent]));

    vi.resetModules();
    const { useCommands: reloaded } = await import("@/stores/commands");
    fetchMock
      .mockResolvedValueOnce(
        Response.json({ ok: true, receipt: fixture.receipt.receipt, status: "accepted" }),
      )
      .mockResolvedValueOnce(
        Response.json({ ...fixture.receipt, replayed: true, status: "accepted" }),
      );
    expect(reloaded.getState().pending[0]?.status).toBe("unchecked");
    await reloaded.getState().check(commandId);
    expect(fetchMock.mock.calls[2]?.[0]).toBe(
      "/sessions/fixture/receipts/send-example?workspace=%2Fwork%2Fkuku",
    );
    expect(fetchMock.mock.calls[3]?.[0]).toBe("/sessions/fixture/commands?replay_only=true");
    expect(postBody(fetchMock, 3)).toEqual(fixture.first_send);
    expect(reloaded.getState().outcome?.result.status).toBe("accepted");
    expect(reloaded.getState().pending).toHaveLength(0);
    expect(sessionStorage.getItem(storedCommands)).toBeNull();
  });

  it("waits for explicit resend after a missing receipt and retains the command identity", async () => {
    vi.resetModules();
    const fetchMock = vi
      .fn()
      .mockRejectedValueOnce(new Error("Disconnected before delivery"))
      .mockResolvedValueOnce(failedReceipt())
      .mockResolvedValueOnce(Response.json(fixture.receipt));
    vi.stubGlobal("fetch", fetchMock);
    const { useCommands } = await import("@/stores/commands");
    await useCommands.getState().submit(intent);
    expect(useCommands.getState().pending[0]?.status).toBe("missing");
    expect(fetchMock).toHaveBeenCalledTimes(2);
    await useCommands.getState().retry(commandId);
    expect(postBody(fetchMock, 2)).toEqual(fixture.first_send);
    expect(useCommands.getState().pending).toHaveLength(0);
  });
});
