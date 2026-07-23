import { spawn, type ChildProcess } from 'node:child_process';
import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer as createHttpServer } from 'node:http';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

import { expect as baseExpect, test as baseTest } from '@playwright/test';

import type {
  CompleteInitRequest,
  CreateTaskRequest,
  CreateTaskResponse,
  InitStatus,
  PlatformStatus,
  RegisterInitialWorkspaceRequest,
  RegistrationRootPage,
  SubmitRunRequest,
  SubmitRunResponse,
  TaskProjection,
  TestProviderRequest,
  UpdateDefaultTierRequest,
  UpdateProvidersRequest,
  WorkspacePage,
} from '../../src/api/generated';
import { createWorkspaces, type WorkspaceFixtures } from './workspaces';

const REPOSITORY_ROOT = resolve(import.meta.dirname, '../../../..');
const DEFAULT_BINARY = resolve(
  REPOSITORY_ROOT,
  'target/debug',
  process.platform === 'win32' ? 'kuku.exe' : 'kuku',
);
const TEST_TOKEN = '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
const TEST_TIER = 'e2e-balanced';

export interface ScenarioHandle {
  readonly name: string;
  readonly runId: string;
  readonly seed: number;
  readonly taskId: string;
}

export interface UnifiedBinary {
  readonly baseUrl: string;
  readonly credential: string;
  readonly gitWorkspace: string;
  readonly plainWorkspace: string;
  readonly home: string;
  readonly releasePackage: boolean;
  readonly scenario: ScenarioHandle;
  stop(): Promise<void>;
}

export interface FreshUnifiedBinary {
  readonly baseUrl: string;
  readonly credential: string;
  readonly providerOrigin: string;
  stop(): Promise<void>;
}

interface DeterministicProvider {
  readonly origin: string;
  stop(): Promise<void>;
}

type TestFixtures = {
  scenarioName: 'full_task' | 'full_task_compact';
  unifiedBinary: UnifiedBinary;
  workspaces: WorkspaceFixtures;
};

async function unusedPort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (address === null || typeof address === 'string') {
        server.close();
        reject(new Error('could not allocate a loopback port'));
        return;
      }
      server.close((error) => {
        if (error === undefined) resolvePort(address.port);
        else reject(error);
      });
    });
  });
}

async function waitForHealth(baseUrl: string, child: ChildProcess): Promise<void> {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`unified binary exited with ${String(child.exitCode)}`);
    }
    try {
      const response = await fetch(`${baseUrl}/health`);
      if (response.ok) return;
    } catch {
      // The listener can still be binding.
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw new Error('timed out waiting for the unified binary health endpoint');
}

async function startDeterministicProvider(): Promise<DeterministicProvider> {
  let messageRequests = 0;
  const server = createHttpServer((request, response) => {
    if (request.method !== 'POST' || request.url !== '/v1/messages') {
      response.writeHead(404).end();
      return;
    }
    let body = '';
    request.setEncoding('utf8');
    request.on('data', (chunk: string) => {
      body += chunk;
    });
    request.on('end', () => {
      let payload: unknown;
      try {
        payload = JSON.parse(body);
      } catch {
        response.writeHead(400).end();
        return;
      }
      response.writeHead(200, {
        Connection: 'close',
        'Content-Type': 'text/event-stream',
      });
      messageRequests += 1;
      const serialized = JSON.stringify(payload);
      const permissionRequest = messageRequests === 3 && !serialized.includes('tool_result');
      response.end(
        (permissionRequest
          ? [
              'event: message_start',
              'data: {"type":"message_start","message":{"id":"msg_release_permission","type":"message","role":"assistant","content":[],"model":"deterministic-fixture","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}',
              '',
              'event: content_block_start',
              'data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_release_permission","name":"run_command","input":{}}}',
              '',
              'event: content_block_delta',
              'data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\\"command\\":\\"printf mobile\\",\\"timeout\\":1,\\"brief\\":\\"verify mobile permission\\"}"}}',
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
            ]
          : [
              'event: message_start',
              'data: {"type":"message_start","message":{"id":"msg_release_e2e","type":"message","role":"assistant","content":[],"model":"deterministic-fixture","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}',
              '',
              'event: content_block_start',
              'data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}',
              '',
              'event: content_block_delta',
              'data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Release package deterministic response."}}',
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
            ]
        ).join('\n'),
      );
    });
  });
  await new Promise<void>((resolveListen, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolveListen);
  });
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('provider did not bind');
  return {
    origin: `http://127.0.0.1:${String(address.port)}`,
    stop: () =>
      new Promise<void>((resolveClose, reject) => {
        server.close((error) => {
          if (error === undefined) resolveClose();
          else reject(error);
        });
      }),
  };
}

async function apiJson<T>(
  baseUrl: string,
  credential: string,
  method: 'GET' | 'POST',
  path: string,
  body?: unknown,
): Promise<T> {
  const response = await fetch(`${baseUrl}/api/v1${path}`, {
    method,
    headers: {
      Accept: 'application/json',
      Authorization: `Bearer ${credential}`,
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok) {
    throw new Error(
      `${method} ${path} returned ${String(response.status)}: ${await response.text()}`,
    );
  }
  return response.json() as Promise<T>;
}

async function initializeScenario(
  baseUrl: string,
  credential: string,
  providerOrigin: string,
  releasePackage: boolean,
): Promise<{ runId: string; taskId: string }> {
  const status = await apiJson<PlatformStatus>(baseUrl, credential, 'GET', '/status');
  if (status.init.phase !== 'required') throw new Error('isolated scenario home was not fresh');

  const providers = {
    expected_revision: status.init.server_revision,
    providers: [
      {
        provider_id: 'e2e-provider',
        format: 'anthropic',
        base_url: providerOrigin,
        credential: { source: 'direct_value', value: 'e2e-provider-key' },
      },
    ],
    tiers: [
      {
        tier_id: TEST_TIER,
        provider_id: 'e2e-provider',
        model: 'deterministic-fixture',
        purpose: 'Embedded browser acceptance',
        think: null,
      },
    ],
  } satisfies UpdateProvidersRequest;
  let init = await apiJson<InitStatus>(baseUrl, credential, 'POST', '/init/providers', providers);

  const defaultTier = {
    expected_revision: init.server_revision,
    tier_id: TEST_TIER,
  } satisfies UpdateDefaultTierRequest;
  init = await apiJson<InitStatus>(baseUrl, credential, 'POST', '/init/default-tier', defaultTier);

  const roots = await apiJson<RegistrationRootPage>(
    baseUrl,
    credential,
    'GET',
    '/registration-roots',
  );
  const root = roots.items[0];
  if (root === undefined) throw new Error('scenario server exposed no registration root');
  const initialWorkspace = {
    workspace: {
      expected_revision: init.server_revision,
      root_id: root.root_id,
      relative_path: 'git-workspace',
      label: 'Git fixture',
    },
  } satisfies RegisterInitialWorkspaceRequest;
  init = await apiJson<InitStatus>(
    baseUrl,
    credential,
    'POST',
    '/init/workspace',
    initialWorkspace,
  );
  const secondWorkspace = {
    workspace: {
      expected_revision: init.server_revision,
      root_id: root.root_id,
      relative_path: 'plain-workspace',
      label: 'Plain fixture',
    },
  } satisfies RegisterInitialWorkspaceRequest;
  init = await apiJson<InitStatus>(baseUrl, credential, 'POST', '/init/workspace', secondWorkspace);

  const probe = {
    expected_revision: init.server_revision,
    tier_id: TEST_TIER,
  } satisfies TestProviderRequest;
  await apiJson<unknown>(baseUrl, credential, 'POST', '/init/test', probe);
  init = await apiJson<InitStatus>(baseUrl, credential, 'GET', '/init/status');
  const complete = { expected_revision: init.server_revision } satisfies CompleteInitRequest;
  const completed = await apiJson<InitStatus>(
    baseUrl,
    credential,
    'POST',
    '/init/complete',
    complete,
  );
  if (!completed.complete || completed.phase !== 'complete') {
    throw new Error(`Init completion returned ${JSON.stringify(completed)}`);
  }
  const confirmed = await apiJson<InitStatus>(baseUrl, credential, 'GET', '/init/status');
  if (!confirmed.complete || confirmed.phase !== 'complete') {
    throw new Error(`Init completion regressed to ${JSON.stringify(confirmed)}`);
  }

  const workspaces = await apiJson<WorkspacePage>(baseUrl, credential, 'GET', '/workspaces');
  const gitWorkspace = workspaces.items.find((workspace) => workspace.label === 'Git fixture');
  if (gitWorkspace === undefined) throw new Error('Git fixture workspace was not registered');
  if (!workspaces.items.some((workspace) => workspace.label === 'Plain fixture')) {
    throw new Error('plain fixture workspace was not registered');
  }

  if (!releasePackage) {
    const hidden = await apiJson<CreateTaskResponse>(baseUrl, credential, 'POST', '/tasks', {
      idempotency_key: 'e2e-create-only-hidden-match',
      workspace_id: gitWorkspace.workspace_id,
    } satisfies CreateTaskRequest);
    await apiJson<SubmitRunResponse>(
      baseUrl,
      credential,
      'POST',
      `/tasks/${encodeURIComponent(hidden.projection.task.task_id)}/runs`,
      {
        expected_task_revision: hidden.projection.task_revision,
        idempotency_key: 'e2e-submit-only-hidden-match',
        message: 'only-hidden-match',
        skill_ids: [],
        tier_id: `tier:${TEST_TIER}`,
      } satisfies SubmitRunRequest,
    );
    for (let offset = 0; offset < 105; offset += 15) {
      await Promise.all(
        Array.from({ length: Math.min(15, 105 - offset) }, (_, index) => {
          const sequence = offset + index;
          return apiJson<CreateTaskResponse>(baseUrl, credential, 'POST', '/tasks', {
            idempotency_key: `e2e-create-search-filler-${String(sequence)}`,
            workspace_id: gitWorkspace.workspace_id,
          } satisfies CreateTaskRequest);
        }),
      );
    }
  }

  const create = {
    idempotency_key: 'e2e-create-full-task',
    workspace_id: gitWorkspace.workspace_id,
  } satisfies CreateTaskRequest;
  const created = await apiJson<CreateTaskResponse>(baseUrl, credential, 'POST', '/tasks', create);
  const submit = {
    expected_task_revision: created.projection.task_revision,
    idempotency_key: 'e2e-submit-full-task',
    message: 'Exercise the full deterministic browser scenario',
    skill_ids: [],
    tier_id: `tier:${TEST_TIER}`,
  } satisfies SubmitRunRequest;
  const accepted = await apiJson<SubmitRunResponse>(
    baseUrl,
    credential,
    'POST',
    `/tasks/${encodeURIComponent(created.projection.task.task_id)}/runs`,
    submit,
  );
  if (releasePackage) {
    const terminalStates = new Set(['completed', 'stopped', 'failed', 'interrupted']);
    const deadline = Date.now() + 20_000;
    while (Date.now() < deadline) {
      const projection = await apiJson<TaskProjection>(
        baseUrl,
        credential,
        'GET',
        `/tasks/${encodeURIComponent(accepted.task_id)}`,
      );
      if (projection.latest_run !== null && terminalStates.has(projection.latest_run.state)) break;
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
    }
    const durable = await apiJson<TaskProjection>(
      baseUrl,
      credential,
      'GET',
      `/tasks/${encodeURIComponent(accepted.task_id)}`,
    );
    if (durable.latest_run === null || !terminalStates.has(durable.latest_run.state)) {
      throw new Error('release package run did not reach a durable terminal state');
    }
  }
  return { runId: accepted.run_id, taskId: accepted.task_id };
}

function serverEnvironment(
  releasePackage: boolean,
  scenarioName: TestFixtures['scenarioName'],
): NodeJS.ProcessEnv {
  const environment = { ...process.env };
  delete environment.KUKU_TEST_SCENARIO;
  delete environment.KUKU_TEST_SEED;
  if (!releasePackage) {
    environment.KUKU_TEST_SCENARIO = scenarioName;
    environment.KUKU_TEST_SEED = '7';
  }
  environment.NO_PROXY = '127.0.0.1,localhost';
  environment.no_proxy = '127.0.0.1,localhost';
  return environment;
}

export async function startFreshUnifiedBinary(
  workspaces: WorkspaceFixtures,
  workerIndex: number,
): Promise<FreshUnifiedBinary> {
  const home = await mkdtemp(join(tmpdir(), `kuku-init-home-${String(workerIndex)}-`));
  const credentialFile = join(home, 'e2e-credential');
  await writeFile(credentialFile, `${TEST_TOKEN}\n`, { mode: 0o600 });
  await chmod(credentialFile, 0o600);

  const provider = await startDeterministicProvider();
  const port = await unusedPort();
  const baseUrl = `http://127.0.0.1:${String(port)}`;
  const binary = process.env.KUKU_E2E_BINARY ?? DEFAULT_BINARY;
  const child = spawn(
    binary,
    [
      'web',
      '--listen',
      `127.0.0.1:${String(port)}`,
      '--auth-token-file',
      credentialFile,
      '--registration-root',
      `Fixtures=${workspaces.registrationRoot}`,
    ],
    {
      cwd: REPOSITORY_ROOT,
      env: { ...serverEnvironment(true, 'full_task_compact'), KUKU_HOME: home },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    },
  );
  const logs: string[] = [];
  child.stdout.on('data', (chunk: Buffer) => logs.push(chunk.toString()));
  child.stderr.on('data', (chunk: Buffer) => logs.push(chunk.toString()));

  let stopped = false;
  const stop = async () => {
    if (stopped) return;
    stopped = true;
    await stopProcess(child);
    await provider.stop();
    await rm(home, { force: true, recursive: true });
  };
  try {
    await waitForHealth(baseUrl, child);
    return {
      baseUrl,
      credential: TEST_TOKEN,
      providerOrigin: provider.origin,
      stop,
    };
  } catch (error) {
    await stop();
    throw new Error(`${error instanceof Error ? error.message : String(error)}\n${logs.join('')}`, {
      cause: error,
    });
  }
}

async function stopProcess(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null) return;
  const exited = new Promise<void>((resolveExit) => {
    child.once('exit', () => {
      resolveExit();
    });
  });
  child.kill('SIGINT');
  const timeout = new Promise<'timeout'>((resolveTimeout) => {
    setTimeout(() => {
      resolveTimeout('timeout');
    }, 3_000);
  });
  if ((await Promise.race([exited.then(() => 'exit' as const), timeout])) === 'timeout') {
    child.kill('SIGTERM');
    await exited;
  }
}

export const test = baseTest.extend<TestFixtures>({
  scenarioName: ['full_task_compact', { option: true }],
  workspaces: [
    async ({ browserName: _browserName }, use, workerInfo) => {
      const root = await mkdtemp(join(tmpdir(), `kuku-e2e-${String(workerInfo.workerIndex)}-`));
      const workspaces = await createWorkspaces(root);
      try {
        await use(workspaces);
      } finally {
        await rm(root, { force: true, recursive: true });
      }
    },
    { scope: 'test' },
  ],
  unifiedBinary: [
    async ({ scenarioName, workspaces }, use, workerInfo) => {
      const releasePackage = process.env.KUKU_E2E_RELEASE_PACKAGE === '1';
      const home = await mkdtemp(join(tmpdir(), `kuku-home-${String(workerInfo.workerIndex)}-`));
      const credentialFile = join(home, 'e2e-credential');
      await writeFile(credentialFile, `${TEST_TOKEN}\n`, { mode: 0o600 });
      await chmod(credentialFile, 0o600);

      const port = await unusedPort();
      const baseUrl = `http://127.0.0.1:${String(port)}`;
      const binary = process.env.KUKU_E2E_BINARY ?? DEFAULT_BINARY;
      const provider = releasePackage ? await startDeterministicProvider() : undefined;
      const child = spawn(
        binary,
        [
          'web',
          '--listen',
          `127.0.0.1:${String(port)}`,
          '--auth-token-file',
          credentialFile,
          '--registration-root',
          `Fixtures=${workspaces.registrationRoot}`,
        ],
        {
          cwd: REPOSITORY_ROOT,
          env: { ...serverEnvironment(releasePackage, scenarioName), KUKU_HOME: home },
          stdio: ['ignore', 'pipe', 'pipe'],
          windowsHide: true,
        },
      );
      const logs: string[] = [];
      child.stdout.on('data', (chunk: Buffer) => logs.push(chunk.toString()));
      child.stderr.on('data', (chunk: Buffer) => logs.push(chunk.toString()));

      try {
        await waitForHealth(baseUrl, child);
        const credential = (await readFile(credentialFile, 'utf8')).trim();
        const status = await fetch(`${baseUrl}/api/v1/status`, {
          headers: { Authorization: `Bearer ${credential}` },
        });
        if (!status.ok) {
          throw new Error(`authenticated status returned ${String(status.status)}`);
        }
        const scenario = await initializeScenario(
          baseUrl,
          credential,
          provider?.origin ?? 'http://127.0.0.1:9',
          releasePackage,
        );
        await use({
          baseUrl,
          credential,
          gitWorkspace: workspaces.gitWorkspace,
          plainWorkspace: workspaces.plainWorkspace,
          home,
          releasePackage,
          scenario: {
            name: releasePackage ? 'release_package' : scenarioName,
            runId: scenario.runId,
            seed: releasePackage ? 0 : 7,
            taskId: scenario.taskId,
          },
          stop: async () => stopProcess(child),
        });
      } catch (error) {
        await stopProcess(child);
        throw new Error(
          `${error instanceof Error ? error.message : String(error)}\n${logs.join('')}`,
          {
            cause: error,
          },
        );
      } finally {
        await stopProcess(child);
        await provider?.stop();
        await rm(home, { force: true, recursive: true });
      }
    },
    { scope: 'test' },
  ],
});

export { baseExpect as expect };
