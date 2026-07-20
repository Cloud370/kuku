import { describe, expect, it, vi } from 'vitest';

import taskStreamEventJson from '../api/generated/fixtures/task_stream_event.json';
import type { Cursor, TaskStreamEvent } from '../api/generated';
import { ContractDecodeError } from '../api/decode';

import { consumeTaskStream } from './subscription';

const taskId = 'tsk_000000000000000000000001';

function streamResponse(chunks: string[]): Response {
  const encoder = new TextEncoder();
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
        controller.close();
      },
    }),
  );
}

describe('consumeTaskStream', () => {
  it('passes the last cursor and decodes split and trailing NDJSON frames', async () => {
    const first = structuredClone(taskStreamEventJson) as TaskStreamEvent;
    const second = structuredClone(first);
    second.cursor += 2;
    const payload = `${JSON.stringify(first)}\n${JSON.stringify(second)}`;
    const subscribe = vi.fn((_taskId: string, _after: Cursor | null, _signal: AbortSignal) =>
      Promise.resolve(streamResponse([payload.slice(0, 37), payload.slice(37)])),
    );
    const frames: TaskStreamEvent[] = [];

    await consumeTaskStream(
      taskId,
      3,
      new AbortController().signal,
      (event) => {
        frames.push(event);
      },
      subscribe,
    );

    expect(subscribe).toHaveBeenCalledWith(taskId, 3, expect.any(AbortSignal));
    expect(frames).toEqual([first, second]);
  });

  it('rejects a frame that fails the generated contract decoder', async () => {
    const invalid = { ...taskStreamEventJson, cursor: 'not-a-number' };
    const subscribe = vi.fn(() =>
      Promise.resolve(streamResponse([`${JSON.stringify(invalid)}\n`])),
    );

    await expect(
      consumeTaskStream(taskId, null, new AbortController().signal, () => undefined, subscribe),
    ).rejects.toBeInstanceOf(ContractDecodeError);
  });

  it('classifies malformed NDJSON as a terminal contract error', async () => {
    const subscribe = vi.fn(() => Promise.resolve(streamResponse(['{"api_version":1\n'])));

    await expect(
      consumeTaskStream(taskId, null, new AbortController().signal, () => undefined, subscribe),
    ).rejects.toBeInstanceOf(ContractDecodeError);
  });
});
