import { createServer as createHttpServer } from 'node:http';

export async function startProvider(defaultText = 'Release candidate acceptance response.') {
  const behaviors = [];
  const enqueuedKeys = [];
  const consumedKeys = [];
  const heldResponses = new Set();
  let responseIndex = 0;
  const server = createHttpServer((request, response) => {
    if (request.method !== 'POST' || request.url !== '/v1/messages') {
      response.writeHead(404).end();
      return;
    }
    let body = '';
    request.setEncoding('utf8');
    request.on('data', (chunk) => {
      body += chunk;
    });
    request.on('end', () => {
      try {
        JSON.parse(body);
      } catch {
        response.writeHead(400).end();
        return;
      }
      responseIndex += 1;
      const behavior = behaviors.shift() ?? { kind: 'text' };
      if (behavior.key !== undefined) consumedKeys.push(behavior.key);
      response.writeHead(200, { Connection: 'close', 'Content-Type': 'text/event-stream' });
      if (behavior.kind === 'hold') {
        heldResponses.add(response);
        response.once('close', () => heldResponses.delete(response));
        response.write(messageStartEvent(responseIndex));
        return;
      }
      response.end(providerEvents(responseIndex, behavior, defaultText));
    });
  });
  await new Promise((resolveListen, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolveListen);
  });
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('provider did not bind');
  return {
    assertConsumed: async () => {
      const deadline = Date.now() + 5_000;
      while (behaviors.length > 0 && Date.now() < deadline) {
        await new Promise((resolveDelay) => setTimeout(resolveDelay, 25));
      }
      if (behaviors.length > 0 || JSON.stringify(consumedKeys) !== JSON.stringify(enqueuedKeys)) {
        throw new Error(
          `provider FIFO mismatch: enqueued=${JSON.stringify(enqueuedKeys)} consumed=${JSON.stringify(consumedKeys)}`,
        );
      }
    },
    enqueue: (...nextBehaviors) => {
      for (const behavior of nextBehaviors) {
        if (typeof behavior.key !== 'string' || behavior.key.length === 0) {
          throw new Error('provider behavior requires a FIFO key');
        }
        enqueuedKeys.push(behavior.key);
        behaviors.push(behavior);
      }
    },
    origin: `http://127.0.0.1:${address.port}`,
    stop: () => {
      for (const response of heldResponses) response.end();
      return new Promise((resolveClose, reject) =>
        server.close((error) => (error === undefined ? resolveClose() : reject(error))),
      );
    },
  };
}

function messageStartEvent(index) {
  return [
    'event: message_start',
    `data: ${JSON.stringify({
      type: 'message_start',
      message: {
        id: `msg_acceptance_${index}`,
        type: 'message',
        role: 'assistant',
        content: [],
        model: 'acceptance-fixture',
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 1, output_tokens: 0 },
      },
    })}`,
    '',
    '',
  ].join('\n');
}

function providerEvents(index, behavior, defaultText) {
  const start = messageStartEvent(index);
  if (behavior.kind === 'tool') {
    const toolId = `tool_acceptance_${index}`;
    return [
      start,
      'event: content_block_start',
      `data: ${JSON.stringify({
        type: 'content_block_start',
        index: 0,
        content_block: { type: 'tool_use', id: toolId, name: behavior.name, input: {} },
      })}`,
      '',
      'event: content_block_delta',
      `data: ${JSON.stringify({
        type: 'content_block_delta',
        index: 0,
        delta: { type: 'input_json_delta', partial_json: JSON.stringify(behavior.input) },
      })}`,
      '',
      'event: content_block_stop',
      'data: {"type":"content_block_stop","index":0}',
      '',
      'event: message_delta',
      'data: {"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":5}}',
      '',
      'event: message_stop',
      'data: {"type":"message_stop"}',
      '',
    ].join('\n');
  }
  return [
    start,
    'event: content_block_start',
    'data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}',
    '',
    'event: content_block_delta',
    `data: ${JSON.stringify({
      type: 'content_block_delta',
      index: 0,
      delta: { type: 'text_delta', text: behavior.text ?? defaultText },
    })}`,
    '',
    'event: content_block_stop',
    'data: {"type":"content_block_stop","index":0}',
    '',
    'event: message_delta',
    'data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":5}}',
    '',
    'event: message_stop',
    'data: {"type":"message_stop"}',
    '',
  ].join('\n');
}
