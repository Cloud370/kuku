import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Entry, Observation, Snapshot, Target } from "@/api/sessions";
import { useConversation } from "./conversation";

const target: Target = { workspace: "/workspace", session: "s1", agent: "main" };
const reviewer = { ...target, agent: "reviewer" };
const encoder = new TextEncoder();

function snapshot(selected: Target, cursor: number): Snapshot {
  return {
    ok: true,
    workspace: selected.workspace,
    session_id: selected.session,
    cursor,
    agent: {
      address: selected.agent,
      creation_event_id: 1,
      status: "running",
      config: {},
      turn: 1,
      last_error: null,
      permission: null,
      question: null,
      usage: { input_tokens: null, output_tokens: null },
    },
    agents: [
      { address: selected.agent, creation_event_id: 1, status: "running", turn: 1, execution_id: 5, pending_inputs: 0 },
    ],
    pending: [],
    selection: null,
  };
}

function message(id: number, agent: string, text: string): Entry {
  return { id, agent, turn: 1, type: "assistant", text };
}

function committed(entry: Entry): Observation {
  return { type: "committed", id: entry.id, agent: entry.agent, kind: "model.response", entry };
}

function observations(frames: Observation[], signal?: AbortSignal | null) {
  return new Response(
    new ReadableStream<Uint8Array>({
      start(controller) {
        for (const frame of frames)
          controller.enqueue(encoder.encode(`${JSON.stringify(frame)}\n`));
        if (signal)
          signal.addEventListener(
            "abort",
            () => {
              controller.close();
            },
            { once: true },
          );
        else controller.close();
      },
    }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("observation lifetime", () => {
  it("starts at the snapshot boundary, reconnects after the last global cursor, and pages backward", async () => {
    const after: string[] = [];
    const before: string[] = [];
    const first = committed(message(9, "main", "New committed text"));
    const other = committed(message(10, "reviewer", "Another agent"));
    const next = committed(message(11, "main", "After reconnect"));
    const fetchMock = vi.fn((path: string, init?: RequestInit) => {
      const url = new URL(path, location.origin);
      if (url.pathname.endsWith("/snapshot"))
        return Promise.resolve(Response.json(snapshot(target, 8)));
      if (url.pathname.endsWith("/history")) {
        const cursor = url.searchParams.get("before");
        before.push(cursor ?? "");
        return Promise.resolve(
          Response.json(
            cursor === "9"
              ? { ok: true, entries: [message(7, "main", "Initial history")], next_before: 7 }
              : { ok: true, entries: [message(3, "main", "Earlier history")], next_before: null },
          ),
        );
      }
      const cursor = url.searchParams.get("after");
      after.push(cursor ?? "");
      return Promise.resolve(
        cursor === "8"
          ? observations([first, first, other])
          : observations([first, next], init?.signal),
      );
    });
    vi.stubGlobal("fetch", fetchMock);
    const { result, unmount } = renderHook(() => useConversation(target));
    try {
      await waitFor(() => {
        expect(result.current.view.entries.map(({ id }) => id)).toEqual([7, 9, 11]);
      });
      expect(after).toEqual(["8", "10"]);
      const preserveScroll = vi.fn();
      result.current.loadOlder(preserveScroll);
      await waitFor(() => {
        expect(result.current.view.entries.map(({ id }) => id)).toEqual([3, 7, 9, 11]);
      });
      expect(before).toEqual(["9", "7"]);
      expect(preserveScroll).toHaveBeenCalledOnce();
    } finally {
      unmount();
    }
  });

  it("aborts the old Agent subscription and ignores a frame that arrives after it was aborted", async () => {
    let oldController: ReadableStreamDefaultController<Uint8Array> | undefined;
    let oldSignal: AbortSignal | null | undefined;
    const fetchMock = vi.fn((path: string, init?: RequestInit) => {
      const url = new URL(path, location.origin);
      const selected = url.searchParams.get("agent") === reviewer.agent ? reviewer : target;
      if (url.pathname.endsWith("/snapshot"))
        return Promise.resolve(Response.json(snapshot(selected, 8)));
      if (url.pathname.endsWith("/history")) {
        return Promise.resolve(
          Response.json({
            ok: true,
            entries: [message(7, selected.agent, selected.agent)],
            next_before: null,
          }),
        );
      }
      if (selected.agent === target.agent) {
        oldSignal = init?.signal;
        return Promise.resolve(
          new Response(
            new ReadableStream<Uint8Array>({
              start(controller) {
                oldController = controller;
              },
            }),
          ),
        );
      }
      return Promise.resolve(
        observations([committed(message(9, selected.agent, "Reviewer response"))], init?.signal),
      );
    });
    vi.stubGlobal("fetch", fetchMock);
    const { result, rerender, unmount } = renderHook(({ selected }) => useConversation(selected), {
      initialProps: { selected: target },
    });
    try {
      await waitFor(() => {
        expect(oldSignal?.aborted).toBe(false);
      });
      rerender({ selected: reviewer });
      expect(oldSignal?.aborted).toBe(true);
      const lateFrame = committed(message(12, target.agent, "Late main response"));
      oldController?.enqueue(encoder.encode(`${JSON.stringify(lateFrame)}\n`));
      await waitFor(() => {
        expect(result.current.view.entries.map(({ agent, id }) => [agent, id])).toEqual([
          ["reviewer", 7],
          ["reviewer", 9],
        ]);
      });
    } finally {
      unmount();
    }
  });
});
