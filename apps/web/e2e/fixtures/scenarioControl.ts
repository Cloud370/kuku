import { expect, type APIRequestContext } from '@playwright/test';

import type { InteractionResponseRequest, TaskProjection } from '../../src/api/generated';
import type { UnifiedBinary } from './unifiedBinary';

function controlHeaders(server: UnifiedBinary): Record<string, string> {
  return { Authorization: `Bearer ${server.credential}` };
}

export async function releaseScenarioBarrier(
  request: APIRequestContext,
  server: UnifiedBinary,
  name: string,
): Promise<void> {
  const response = await request.post(
    `${server.baseUrl}/api/v1/testing/barriers/${encodeURIComponent(name)}/release`,
    { headers: controlHeaders(server) },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
}

export async function failScenarioBarrier(
  request: APIRequestContext,
  server: UnifiedBinary,
  name: string,
  reason: string,
): Promise<void> {
  const response = await request.post(
    `${server.baseUrl}/api/v1/testing/failures/${encodeURIComponent(name)}`,
    { data: { reason }, headers: controlHeaders(server) },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
}

export async function expectScenarioControlsRequireAuthentication(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<void> {
  const unauthenticated = await request.post(
    `${server.baseUrl}/api/v1/testing/barriers/after-tool/release`,
  );
  expect(unauthenticated.status()).toBe(401);
  const unknown = await request.post(
    `${server.baseUrl}/api/v1/testing/barriers/not-a-fixture-barrier/release`,
    { headers: controlHeaders(server) },
  );
  expect(unknown.status()).toBe(404);
}

async function projection(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<TaskProjection> {
  const response = await request.get(
    `${server.baseUrl}/api/v1/tasks/${encodeURIComponent(server.scenario.taskId)}`,
    { headers: controlHeaders(server) },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  return response.json() as Promise<TaskProjection>;
}

export async function respondToPendingInteraction(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<void> {
  await expect
    .poll(async () => {
      const current = await projection(request, server);
      return pendingInteraction(current)?.item.interaction_id ?? null;
    })
    .not.toBeNull();
  const current = await projection(request, server);
  const interaction = pendingInteraction(current);
  if (interaction === null) throw new Error('scenario exposed no interaction');
  const choice = interaction.item.choices[0];
  if (choice === undefined) throw new Error('scenario interaction exposed no choice');
  const body = {
    choice_id: choice.choice_id,
    expected_task_revision: current.task_revision,
    idempotency_key: `e2e-interaction-${interaction.item.interaction_id}`,
  } satisfies InteractionResponseRequest;
  const response = await request.post(
    `${server.baseUrl}/api/v1/tasks/${encodeURIComponent(server.scenario.taskId)}/interactions/${encodeURIComponent(interaction.item.interaction_id)}`,
    { data: body, headers: controlHeaders(server) },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
}

function pendingInteraction(
  current: TaskProjection,
): Extract<TaskProjection['timeline'][number], { type: 'interaction' }> | null {
  return (
    current.timeline.find(
      (item): item is Extract<typeof item, { type: 'interaction' }> =>
        item.type === 'interaction' && item.item.status === 'pending',
    ) ?? null
  );
}

export async function completeScenario(
  request: APIRequestContext,
  server: UnifiedBinary,
): Promise<void> {
  await releaseScenarioBarrier(request, server, 'after-tool');
  await respondToPendingInteraction(request, server);
  await releaseScenarioBarrier(request, server, 'continuity-before-finish');
  await expect
    .poll(async () => (await projection(request, server)).task.state)
    .toMatch(/completed|stopped|failed|interrupted/);
}
