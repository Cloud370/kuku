import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { chmod, lstat, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { isIP } from 'node:net';
import { tmpdir } from 'node:os';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { promisify } from 'node:util';

import { startProvider } from './acceptance-provider.mjs';

const execFileAsync = promisify(execFile);
const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repositoryRoot = resolve(webRoot, '../..');
const humanScenarioPath = join(
  repositoryRoot,
  'crates/kuku-server/tests/fixtures/scenarios/human_acceptance.json',
);
const token = 'abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789';

function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
}

async function existing(path) {
  try {
    await lstat(path);
    return true;
  } catch {
    return false;
  }
}

async function collectFiles(root, path = root) {
  if (!(await existing(path))) return [];
  const info = await lstat(path);
  if (info.isFile()) return [path];
  const entries = await readdir(path);
  const nested = await Promise.all(entries.map((entry) => collectFiles(root, join(path, entry))));
  return nested
    .flat()
    .sort((left, right) => relative(root, left).localeCompare(relative(root, right)));
}

export async function hashPath(path) {
  const info = (await existing(path)) ? await lstat(path) : null;
  if (info?.isFile()) return sha256(await readFile(path));
  const hash = createHash('sha256');
  for (const file of await collectFiles(path)) {
    hash.update(relative(path, file).replaceAll('\\', '/'));
    hash.update('\0');
    hash.update(await readFile(file));
    hash.update('\0');
  }
  return hash.digest('hex');
}

export async function hashPaths(paths) {
  const hash = createHash('sha256');
  for (const path of [...paths].sort()) {
    hash.update(basename(path));
    hash.update('\0');
    hash.update(await readFile(path));
    hash.update('\0');
  }
  return hash.digest('hex');
}

export async function hashAcceptanceHarness() {
  return hashPaths([
    fileURLToPath(import.meta.url),
    fileURLToPath(new URL('./acceptance-provider.mjs', import.meta.url)),
  ]);
}

export function validateListen(value, formal) {
  const match = value.match(/^(.+):(\d+)$/);
  if (match === null) throw new Error('listen must use HOST:PORT');
  const host = match[1]?.replace(/^\[(.*)\]$/, '$1') ?? '';
  const port = Number(match[2]);
  if (!Number.isInteger(port) || port < 0 || port > 65535)
    throw new Error('listen port is invalid');
  if (formal) {
    const loopback = host === 'localhost' || host === '127.0.0.1' || host === '::1';
    const wildcard = host === '0.0.0.0' || host === '::';
    if (isIP(host) === 0 || loopback || wildcard) {
      throw new Error('formal Human gates require a reachable non-loopback listen address');
    }
  }
  return value;
}

export function buildCandidateIdentity(input) {
  const identity = {
    source_sha: input.sourceSha,
    binary_hash: input.binaryHash,
    assets_hash: input.assetsHash,
    generated_contract_hash: input.contractHash,
    visual_baseline_hash: input.baselineHash,
    scenario_hash: input.scenarioHash,
    harness_hash: input.harnessHash,
    scenario: input.scenario,
    seed: input.seed,
    ...(input.archiveHash === undefined ? {} : { archive_hash: input.archiveHash }),
  };
  return { ...identity, build_hash: sha256(JSON.stringify(identity)) };
}

export function buildCandidateManifest(input) {
  const identity = buildCandidateIdentity(input);
  const tasks = input.tasks.map((task) => ({
    path: `/tasks/${encodeURIComponent(task.task_id)}`,
    ...(task.purpose === undefined ? {} : { purpose: task.purpose }),
    state: task.state,
    title: task.title,
  }));
  const deepLinks = input.gate === 'h1-h2' ? h1H2DeepLinks(tasks) : h3DeepLinks(tasks);
  return {
    gate: input.gate,
    ...identity,
    url: `${input.origin}/#credential=${encodeURIComponent(input.credential)}`,
    credential: input.credential,
    deep_links: deepLinks,
  };
}

function h1H2DeepLinks(tasks) {
  const h1Purposes = ['workbench-running', 'workbench-needs-attention', 'workbench-completed'];
  const h2Purposes = [
    'context-current',
    'context-historical',
    'context-drifted',
    'context-unavailable',
  ];
  const expectedStates = new Map([
    ['workbench-running', 'running'],
    ['workbench-needs-attention', 'needs_attention'],
    ['workbench-completed', 'completed'],
    ['context-current', 'completed'],
    ['context-historical', 'completed'],
    ['context-drifted', 'completed'],
    ['context-unavailable', 'draft'],
  ]);
  const byPurpose = new Map();
  for (const task of tasks) {
    if (task.purpose === undefined) continue;
    if (byPurpose.has(task.purpose)) {
      throw new Error(`acceptance task purpose is duplicated: ${task.purpose}`);
    }
    const expectedState = expectedStates.get(task.purpose);
    if (expectedState !== undefined && task.state !== expectedState) {
      throw new Error(
        `acceptance task ${task.purpose} is ${task.state}; expected ${expectedState}`,
      );
    }
    byPurpose.set(task.purpose, task.path);
  }
  const paths = (purposes) =>
    purposes.map((purpose) => {
      const path = byPurpose.get(purpose);
      if (path === undefined) throw new Error(`acceptance task purpose is missing: ${purpose}`);
      return path;
    });
  const h1 = paths(h1Purposes);
  const h2 = paths(h2Purposes);
  if (new Set([...h1, ...h2]).size !== h1.length + h2.length) {
    throw new Error('acceptance task purposes must use distinct durable Tasks');
  }
  return { h1, h2 };
}

function h3DeepLinks(tasks) {
  return { h3: [...tasks.map((task) => `${task.path}/review`), '/settings', '/guide'] };
}

export function buildServerEnvironment(gate, seed, baseEnvironment = process.env) {
  const environment = { ...baseEnvironment };
  delete environment.KUKU_TEST_SCENARIO;
  delete environment.KUKU_TEST_SEED;
  void gate;
  void seed;
  environment.NO_PROXY = '127.0.0.1,localhost';
  environment.no_proxy = '127.0.0.1,localhost';
  return environment;
}

export function resolveH3CandidateIdentity(a5Manifest, sourceSha, archiveHash) {
  if (a5Manifest?.source_sha !== sourceSha)
    throw new Error('A5 source SHA does not match checkout');
  const promotion = a5Manifest?.promotion_manifest;
  if (promotion?.source_sha !== sourceSha) {
    throw new Error('A5 promotion source SHA does not match checkout');
  }
  if (a5Manifest?.harness_hash !== promotion?.harness_hash) {
    throw new Error('A5 harness identity does not match promotion');
  }
  const archiveListed = Array.isArray(a5Manifest?.package_sha256sums)
    ? a5Manifest.package_sha256sums.some((line) => line.trim().split(/\s+/)[0] === archiveHash)
    : false;
  if (!archiveListed) throw new Error('archive hash is not present in the A5 candidate manifest');
  const required = [
    'assets_hash',
    'generated_contract_hash',
    'visual_baseline_hash',
    'scenario_hash',
    'harness_hash',
    'scenario',
    'seed',
  ];
  for (const field of required) {
    if (promotion[field] === undefined || promotion[field] === null) {
      throw new Error(`A5 promotion manifest is missing ${field}`);
    }
  }
  return {
    sourceSha,
    assetsHash: promotion.assets_hash,
    contractHash: promotion.generated_contract_hash,
    baselineHash: promotion.visual_baseline_hash,
    scenarioHash: promotion.scenario_hash,
    harnessHash: promotion.harness_hash,
    scenario: promotion.scenario,
    seed: promotion.seed,
  };
}

function parseArguments(argv) {
  const options = {
    artifact: null,
    candidateManifest: null,
    gate: 'h1-h2',
    listen: '127.0.0.1:0',
    listenExplicit: false,
    seed: 7,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const option = argv[index];
    const value = argv[index + 1];
    if (option === '--gate' && value !== undefined) options.gate = value;
    else if (option === '--artifact' && value !== undefined) options.artifact = resolve(value);
    else if (option === '--candidate-manifest' && value !== undefined)
      options.candidateManifest = resolve(value);
    else if (option === '--listen' && value !== undefined) {
      options.listen = value;
      options.listenExplicit = true;
    } else if (option === '--seed' && value !== undefined) options.seed = Number(value);
    else throw new Error(`unknown or incomplete option: ${option}`);
    index += 1;
  }
  if (!['h1-h2', 'h3'].includes(options.gate)) throw new Error('gate must be h1-h2 or h3');
  if (options.gate === 'h3' && options.artifact === null) throw new Error('H3 requires --artifact');
  if (options.gate === 'h3' && options.candidateManifest === null)
    throw new Error('H3 requires --candidate-manifest');
  if (!Number.isSafeInteger(options.seed) || options.seed < 0)
    throw new Error('seed must be a safe integer');
  validateListen(options.listen, options.listenExplicit);
  return options;
}

async function extractArtifact(artifact, destination) {
  await mkdir(destination, { recursive: true });
  if (artifact.endsWith('.zip')) await execFileAsync('unzip', ['-q', artifact, '-d', destination]);
  else await execFileAsync('tar', ['-xf', artifact, '-C', destination]);
  const files = await collectFiles(destination);
  const expected = process.platform === 'win32' ? 'kuku.exe' : 'kuku';
  const binary = files.find((file) => basename(file) === expected);
  if (binary === undefined) throw new Error(`artifact does not contain ${expected}`);
  return binary;
}

async function prepareBinary(options, scratch) {
  if (options.gate === 'h3') return extractArtifact(options.artifact, join(scratch, 'artifact'));
  await rm(join(webRoot, 'dist'), { force: true, recursive: true });
  await execFileAsync('npm', ['run', 'build'], { cwd: webRoot, maxBuffer: 10 * 1024 * 1024 });
  await execFileAsync(
    'cargo',
    ['build', '-p', 'kuku-app', '--features', 'embedded-web-assets,test-scenarios'],
    { cwd: repositoryRoot, maxBuffer: 10 * 1024 * 1024 },
  );
  return join(
    repositoryRoot,
    'target',
    'debug',
    process.platform === 'win32' ? 'kuku.exe' : 'kuku',
  );
}

async function waitForOrigin(child, preferredHost) {
  return new Promise((resolveOrigin, reject) => {
    let logs = '';
    const timeout = setTimeout(
      () => reject(new Error(`server did not report an origin\n${logs}`)),
      20_000,
    );
    const inspect = (chunk) => {
      logs += chunk.toString();
      const urls = [...logs.matchAll(/kuku (?:server|LAN): (http:\/\/[^\s]+)/g)].map(
        (match) => match[1],
      );
      const selected = urls.find((url) => new URL(url).hostname === preferredHost) ?? urls[0];
      if (selected !== undefined) {
        clearTimeout(timeout);
        resolveOrigin(selected);
      }
    };
    child.stdout.on('data', inspect);
    child.stderr.on('data', (chunk) => {
      logs += chunk.toString();
    });
    child.once('exit', (code) => {
      clearTimeout(timeout);
      reject(new Error(`server exited with ${code}\n${logs}`));
    });
  });
}

async function apiJson(origin, method, path, body) {
  const response = await fetch(`${origin}/api/v1${path}`, {
    method,
    headers: {
      Accept: 'application/json',
      Authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok)
    throw new Error(`${method} ${path} returned ${response.status}: ${await response.text()}`);
  return response.json();
}

async function waitForTerminalTask(origin, taskId) {
  const terminal = new Set(['completed', 'stopped', 'failed', 'interrupted']);
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const projection = await apiJson(origin, 'GET', `/tasks/${encodeURIComponent(taskId)}`);
    if (projection.latest_run !== null && terminal.has(projection.latest_run.state)) {
      return projection;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw new Error(`Task ${taskId} did not reach a terminal state`);
}

async function submitReleaseTask(origin, workspaceId, key, message) {
  const created = await apiJson(origin, 'POST', '/tasks', {
    idempotency_key: `h3-create-${key}`,
    workspace_id: workspaceId,
  });
  await apiJson(
    origin,
    'POST',
    `/tasks/${encodeURIComponent(created.projection.task.task_id)}/runs`,
    {
      expected_task_revision: created.projection.task_revision,
      idempotency_key: `h3-submit-${key}`,
      message,
      skill_ids: [],
      tier_id: 'tier:acceptance-balanced',
    },
  );
  return waitForTerminalTask(origin, created.projection.task.task_id);
}

async function initializePlatform(origin, providerOrigin, registrations) {
  const platform = await apiJson(origin, 'GET', '/status');
  let init = await apiJson(origin, 'POST', '/init/providers', {
    expected_revision: platform.init.server_revision,
    providers: [
      {
        provider_id: 'acceptance-provider',
        format: 'anthropic',
        base_url: providerOrigin,
        credential: { source: 'direct_value', value: 'acceptance-key' },
      },
    ],
    tiers: [
      {
        tier_id: 'acceptance-balanced',
        provider_id: 'acceptance-provider',
        model: 'acceptance-fixture',
        purpose: 'Release candidate acceptance',
        think: null,
      },
    ],
  });
  init = await apiJson(origin, 'POST', '/init/default-tier', {
    expected_revision: init.server_revision,
    tier_id: 'acceptance-balanced',
  });
  const roots = await apiJson(origin, 'GET', '/registration-roots');
  const root = roots.items?.[0];
  if (root === undefined) throw new Error('release candidate exposed no registration root');
  for (const workspace of registrations) {
    init = await apiJson(origin, 'POST', '/init/workspace', {
      workspace: {
        expected_revision: init.server_revision,
        root_id: root.root_id,
        ...workspace,
      },
    });
  }
  await apiJson(origin, 'POST', '/init/test', {
    expected_revision: init.server_revision,
    tier_id: 'acceptance-balanced',
  });
  init = await apiJson(origin, 'GET', '/init/status');
  await apiJson(origin, 'POST', '/init/complete', { expected_revision: init.server_revision });
  return apiJson(origin, 'GET', '/workspaces');
}

async function initializeReleaseCandidate(origin, providerOrigin, workspaceRoot) {
  const workspaces = await initializePlatform(origin, providerOrigin, [
    { relative_path: 'git-workspace', label: 'Git acceptance' },
    { relative_path: 'plain-workspace', label: 'Plain acceptance' },
  ]);
  const gitWorkspace = workspaces.items?.find((workspace) => workspace.label === 'Git acceptance');
  const plainWorkspace = workspaces.items?.find(
    (workspace) => workspace.label === 'Plain acceptance',
  );
  if (gitWorkspace === undefined || plainWorkspace === undefined) {
    throw new Error('release candidate workspaces were not registered');
  }
  const [gitTask, plainTask, outdatedTask] = await Promise.all([
    submitReleaseTask(
      origin,
      gitWorkspace.workspace_id,
      'git-review',
      'Git Review release candidate',
    ),
    submitReleaseTask(
      origin,
      plainWorkspace.workspace_id,
      'plain-review',
      'Non-Git Review release candidate',
    ),
    submitReleaseTask(
      origin,
      gitWorkspace.workspace_id,
      'outdated-review',
      'Outdated Review release candidate',
    ),
  ]);

  const changeQuery = new URLSearchParams({ limit: '100' });
  const changes = await apiJson(
    origin,
    'GET',
    `/workspaces/${encodeURIComponent(gitWorkspace.workspace_id)}/changes?${changeQuery}`,
  );
  const change = changes.entries?.[0];
  if (change === undefined) throw new Error('Git acceptance workspace exposed no change');
  const diffQuery = new URLSearchParams({
    cursor: '',
    limit: '4000',
    path: change.path,
    revision: change.revision,
  });
  diffQuery.delete('cursor');
  const diff = await apiJson(
    origin,
    'GET',
    `/workspaces/${encodeURIComponent(gitWorkspace.workspace_id)}/changes/diff?${diffQuery}`,
  );
  const line = diff.hunks
    ?.flatMap((hunk) => hunk.lines)
    .find((candidate) => candidate.new_line !== null);
  if (line === undefined) throw new Error('Git acceptance diff exposed no annotatable line');
  await apiJson(
    origin,
    'POST',
    `/tasks/${encodeURIComponent(outdatedTask.task.task_id)}/review/annotations`,
    {
      expected_task_revision: outdatedTask.task_revision,
      idempotency_key: 'h3-outdated-annotation',
      notes: [
        {
          comment: 'This anchor becomes outdated for H3 review.',
          end_line: line.new_line,
          excerpt: line.text,
          path: change.path,
          revision: diff.revision,
          side: 'new',
          start_line: line.new_line,
        },
      ],
    },
  );
  await writeFile(
    join(workspaceRoot, 'git-workspace', 'review.txt'),
    'release candidate changed again\n',
  );
  const submissions = await apiJson(
    origin,
    'GET',
    `/tasks/${encodeURIComponent(outdatedTask.task.task_id)}/review/submissions?limit=50`,
  );
  if (submissions.items?.[0]?.notes?.[0]?.status !== 'outdated') {
    throw new Error('release candidate annotation did not become outdated');
  }
  return [
    gitTask.task,
    plainTask.task,
    (await apiJson(origin, 'GET', `/tasks/${encodeURIComponent(outdatedTask.task.task_id)}`)).task,
  ];
}

async function waitForTaskState(origin, taskId, expected) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const projection = await apiJson(origin, 'GET', `/tasks/${encodeURIComponent(taskId)}`);
    if (expected.has(projection.task.state)) return projection;
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw new Error(`Task ${taskId} did not reach ${[...expected].join(' or ')}`);
}

async function createAcceptanceTask(origin, workspaceId, key) {
  return apiJson(origin, 'POST', '/tasks', {
    idempotency_key: `human-create-${key}`,
    workspace_id: workspaceId,
  });
}

async function submitAcceptanceRun(origin, provider, projection, key, message, behaviors) {
  provider.enqueue(
    ...behaviors.map((behavior, index) => ({
      ...behavior,
      key: `${key}:${index + 1}`,
    })),
  );
  return apiJson(origin, 'POST', `/tasks/${encodeURIComponent(projection.task.task_id)}/runs`, {
    expected_task_revision: projection.task_revision,
    idempotency_key: `human-submit-${key}`,
    message,
    skill_ids: [],
    tier_id: 'tier:acceptance-balanced',
  });
}

function acceptanceTask(purpose, projection) {
  return { ...projection.task, purpose };
}

async function completedAcceptanceTask(
  origin,
  provider,
  workspaceId,
  purpose,
  key,
  message,
  behaviors,
) {
  const created = await createAcceptanceTask(origin, workspaceId, key);
  const accepted = await submitAcceptanceRun(
    origin,
    provider,
    created.projection,
    key,
    message,
    behaviors,
  );
  const terminal = await waitForTaskState(
    origin,
    accepted.task_id,
    new Set(['completed', 'stopped', 'failed', 'interrupted']),
  );
  if (terminal.task.state !== 'completed') {
    throw new Error(`${purpose} ended in ${terminal.task.state}`);
  }
  return terminal;
}

async function initializeHumanScenario(origin, provider, workspaceRoot, scenario) {
  const workspaces = await initializePlatform(origin, provider.origin, [
    { relative_path: 'git-workspace', label: 'Git acceptance' },
    { relative_path: 'plain-workspace', label: 'Plain acceptance' },
  ]);
  const workspace = workspaces.items?.find((item) => item.label === 'Git acceptance');
  if (workspace === undefined) throw new Error('human candidate workspace was not registered');
  const message = (purpose) => `${scenario.task.message} (${purpose.replaceAll('-', ' ')})`;
  const textBehavior = () => ({ kind: 'text', text: scenario.provider.response });
  const observedFile = scenario.workspace.working_files[0];
  if (observedFile === undefined) throw new Error('human scenario has no observed workspace file');

  const completed = await completedAcceptanceTask(
    origin,
    provider,
    workspace.workspace_id,
    'workbench-completed',
    'workbench-completed',
    message('workbench-completed'),
    [textBehavior()],
  );
  const current = await completedAcceptanceTask(
    origin,
    provider,
    workspace.workspace_id,
    'context-current',
    'context-current',
    message('context-current'),
    [textBehavior()],
  );

  const historicalCreated = await createAcceptanceTask(
    origin,
    workspace.workspace_id,
    'context-historical',
  );
  const historicalFirst = await submitAcceptanceRun(
    origin,
    provider,
    historicalCreated.projection,
    'context-historical-1',
    `${message('context-historical')} First Request.`,
    [textBehavior()],
  );
  const historicalFirstTerminal = await waitForTaskState(
    origin,
    historicalFirst.task_id,
    new Set(['completed']),
  );
  const historicalSecond = await submitAcceptanceRun(
    origin,
    provider,
    historicalFirstTerminal,
    'context-historical-2',
    `${message('context-historical')} Second Request.`,
    [textBehavior()],
  );
  const historical = await waitForTaskState(
    origin,
    historicalSecond.task_id,
    new Set(['completed']),
  );
  const historicalContext = await apiJson(
    origin,
    'GET',
    `/tasks/${encodeURIComponent(historical.task.task_id)}/context`,
  );
  if (historicalContext.request_history?.length < 2) {
    throw new Error('historical Context did not preserve two immutable Requests');
  }

  const drifted = await completedAcceptanceTask(
    origin,
    provider,
    workspace.workspace_id,
    'context-drifted',
    'context-drifted',
    message('context-drifted'),
    [{ kind: 'tool', name: 'read_file', input: { path: observedFile.path } }, textBehavior()],
  );
  await mkdir(
    dirname(join(workspaceRoot, 'git-workspace', scenario.workspace.revision_update.path)),
    {
      recursive: true,
    },
  );
  await writeFile(
    join(workspaceRoot, 'git-workspace', scenario.workspace.revision_update.path),
    scenario.workspace.revision_update.content,
  );
  const driftedContext = await apiJson(
    origin,
    'GET',
    `/tasks/${encodeURIComponent(drifted.task.task_id)}/context`,
  );
  if ((driftedContext.health?.source_drift_count ?? 0) < 1) {
    throw new Error('drifted Context did not report source drift');
  }

  const unavailable = await createAcceptanceTask(
    origin,
    workspace.workspace_id,
    'context-unavailable',
  );
  const unavailableContext = await apiJson(
    origin,
    'GET',
    `/tasks/${encodeURIComponent(unavailable.projection.task.task_id)}/context`,
  );
  if (unavailableContext.selected_request !== null) {
    throw new Error('unavailable Context unexpectedly exposed a Request');
  }

  const attentionCreated = await createAcceptanceTask(
    origin,
    workspace.workspace_id,
    'workbench-needs-attention',
  );
  const attentionAccepted = await submitAcceptanceRun(
    origin,
    provider,
    attentionCreated.projection,
    'workbench-needs-attention',
    message('workbench-needs-attention'),
    [
      {
        kind: 'tool',
        name: 'run_command',
        input: {
          command: 'printf acceptance',
          timeout: 1,
          brief: 'Review deterministic permission state',
        },
      },
    ],
  );
  const attention = await waitForTaskState(
    origin,
    attentionAccepted.task_id,
    new Set(['needs_attention']),
  );

  const runningCreated = await createAcceptanceTask(
    origin,
    workspace.workspace_id,
    'workbench-running',
  );
  const runningAccepted = await submitAcceptanceRun(
    origin,
    provider,
    runningCreated.projection,
    'workbench-running',
    message('workbench-running'),
    [{ kind: 'hold' }],
  );
  const running = await waitForTaskState(origin, runningAccepted.task_id, new Set(['running']));
  await provider.assertConsumed();

  return [
    acceptanceTask('workbench-running', running),
    acceptanceTask('workbench-needs-attention', attention),
    acceptanceTask('workbench-completed', completed),
    acceptanceTask('context-current', current),
    acceptanceTask('context-historical', historical),
    acceptanceTask('context-drifted', drifted),
    acceptanceTask('context-unavailable', unavailable.projection),
  ];
}

async function stopChild(child) {
  if (child.exitCode !== null) return;
  child.kill('SIGINT');
  await Promise.race([
    new Promise((resolveExit) => child.once('exit', resolveExit)),
    new Promise((resolveTimeout) => setTimeout(resolveTimeout, 5_000)),
  ]);
  if (child.exitCode === null) child.kill('SIGTERM');
}

async function materializeScenarioFiles(root, scenario) {
  for (const file of [...scenario.workspace.baseline_files, ...scenario.workspace.working_files]) {
    const target = resolve(root, file.path);
    const contained = relative(root, target);
    if (contained.startsWith('..') || contained === '') {
      throw new Error(`human scenario workspace path is invalid: ${file.path}`);
    }
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, file.content);
  }
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const humanScenario = JSON.parse(await readFile(humanScenarioPath, 'utf8'));
  const scratch = await mkdtemp(join(tmpdir(), 'kuku-human-acceptance-'));
  const home = join(scratch, 'home');
  const roots = join(scratch, 'workspaces');
  await mkdir(home, { recursive: true });
  await mkdir(join(roots, 'git-workspace'), { recursive: true });
  await mkdir(join(roots, 'plain-workspace'), { recursive: true });
  if (options.gate === 'h3') {
    await writeFile(join(roots, 'git-workspace', 'review.txt'), 'release candidate review\n');
    await writeFile(join(roots, 'plain-workspace', 'notes.txt'), 'plain acceptance workspace\n');
  } else {
    await materializeScenarioFiles(join(roots, 'git-workspace'), humanScenario);
    await writeFile(join(roots, 'plain-workspace', 'notes.txt'), 'Acceptance notes\n');
  }
  await execFileAsync('git', ['init', '--initial-branch=main'], {
    cwd: join(roots, 'git-workspace'),
  });
  await execFileAsync('git', ['add', '.'], { cwd: join(roots, 'git-workspace') });
  await execFileAsync(
    'git',
    [
      '-c',
      'user.name=Kuku Acceptance',
      '-c',
      'user.email=acceptance@example.invalid',
      'commit',
      '-m',
      'baseline',
    ],
    { cwd: join(roots, 'git-workspace') },
  );
  if (options.gate === 'h3') {
    await writeFile(join(roots, 'git-workspace', 'review.txt'), 'release candidate changed\n');
  }
  const credentialFile = join(home, 'credential');
  await writeFile(credentialFile, `${token}\n`, { mode: 0o600 });
  await chmod(credentialFile, 0o600);
  const binary = await prepareBinary(options, scratch);
  const provider = await startProvider(
    options.gate === 'h1-h2' ? humanScenario.provider.response : undefined,
  );
  const child = spawn(
    binary,
    [
      'web',
      '--listen',
      options.listen,
      '--auth-token-file',
      credentialFile,
      '--registration-root',
      `Acceptance=${roots}`,
    ],
    {
      cwd: repositoryRoot,
      env: { ...buildServerEnvironment(options.gate, options.seed), KUKU_HOME: home },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    },
  );
  const preferredHost = options.listen
    .slice(0, options.listen.lastIndexOf(':'))
    .replace(/^\[(.*)\]$/, '$1');
  try {
    const origin = await waitForOrigin(child, preferredHost);
    const tasks =
      options.gate === 'h3'
        ? await initializeReleaseCandidate(origin, provider.origin, roots)
        : await initializeHumanScenario(origin, provider, roots, humanScenario);
    const checkoutSha = (
      await execFileAsync('git', ['rev-parse', 'HEAD'], { cwd: repositoryRoot })
    ).stdout.trim();
    const archiveHash = options.artifact === null ? undefined : await hashPath(options.artifact);
    const h3Identity =
      options.gate === 'h3'
        ? resolveH3CandidateIdentity(
            JSON.parse(await readFile(options.candidateManifest, 'utf8')),
            checkoutSha,
            archiveHash,
          )
        : null;
    const manifest = buildCandidateManifest({
      gate: options.gate,
      sourceSha: h3Identity?.sourceSha ?? checkoutSha,
      binaryHash: await hashPath(binary),
      assetsHash: h3Identity?.assetsHash ?? (await hashPath(join(webRoot, 'dist'))),
      contractHash:
        h3Identity?.contractHash ??
        (await hashPath(join(webRoot, 'src/api/generated/manifest.json'))),
      baselineHash:
        h3Identity?.baselineHash ?? (await hashPath(join(webRoot, 'e2e/__screenshots__'))),
      scenarioHash: h3Identity?.scenarioHash ?? (await hashPath(humanScenarioPath)),
      harnessHash: h3Identity?.harnessHash ?? (await hashAcceptanceHarness()),
      scenario: h3Identity?.scenario ?? humanScenario.name,
      origin,
      credential: token,
      seed: h3Identity?.seed ?? options.seed,
      tasks,
      ...(archiveHash === undefined ? {} : { archiveHash }),
    });
    const manifestPath = join(scratch, 'acceptance-manifest.json');
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, { mode: 0o600 });
    console.log(`Candidate manifest: ${manifestPath}`);
    console.log(JSON.stringify(manifest, null, 2));
    await new Promise((resolveStop) => {
      process.once('SIGINT', resolveStop);
      process.once('SIGTERM', resolveStop);
      child.once('exit', resolveStop);
    });
  } finally {
    await stopChild(child);
    await provider.stop();
    await rm(scratch, { force: true, recursive: true });
  }
}

if (process.argv[1] !== undefined && pathToFileURL(process.argv[1]).href === import.meta.url) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
