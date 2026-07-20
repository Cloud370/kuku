import type { Cursor, TaskStreamEvent } from '../api/generated';
import { webApi } from '../api/client';
import { ContractDecodeError, decodeTaskStreamEvent } from '../api/decode';

export type TaskSubscribe = typeof webApi.tasks.subscribe;

export async function consumeTaskStream(
  taskId: string,
  after: Cursor | null,
  signal: AbortSignal,
  onEvent: (event: TaskStreamEvent) => void,
  subscribe: TaskSubscribe = webApi.tasks.subscribe,
): Promise<void> {
  const response = await subscribe(taskId, after, signal);
  if (response.body === null) throw new Error('Task subscription response has no body');

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';

  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split('\n');
    buffer = lines.pop() ?? '';
    for (const line of lines) emitLine(line, onEvent);
  }

  buffer += decoder.decode();
  emitLine(buffer, onEvent);
}

function emitLine(line: string, onEvent: (event: TaskStreamEvent) => void): void {
  if (line.trim().length === 0) return;
  let value: unknown;
  try {
    value = JSON.parse(line) as unknown;
  } catch {
    throw new ContractDecodeError(['/ must be valid JSON']);
  }
  onEvent(decodeTaskStreamEvent(value));
}
