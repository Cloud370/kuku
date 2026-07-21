import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import {
  buildCandidateIdentity,
  buildCandidateManifest,
  buildServerEnvironment,
  hashAcceptanceHarness,
  hashPath,
  hashPaths,
  resolveH3CandidateIdentity,
  validateListen,
} from './acceptance-human.mjs';

const repositoryRoot = join(import.meta.dirname, '../../..');

test('binds H1 and H2 links to one complete candidate identity', async () => {
  const tasks = [
    ['workbench-running', 'running'],
    ['workbench-needs-attention', 'needs_attention'],
    ['workbench-completed', 'completed'],
    ['context-current', 'completed'],
    ['context-historical', 'completed'],
    ['context-drifted', 'completed'],
    ['context-unavailable', 'draft'],
  ].map(([purpose, state]) => ({
    purpose,
    state,
    task_id: `tsk_${purpose}`,
    title: purpose,
  }));
  const manifest = buildCandidateManifest({
    gate: 'h1-h2',
    sourceSha: 'a'.repeat(40),
    binaryHash: 'b'.repeat(64),
    assetsHash: 'c'.repeat(64),
    contractHash: 'd'.repeat(64),
    baselineHash: 'e'.repeat(64),
    scenarioHash: 'f'.repeat(64),
    harnessHash: '1'.repeat(64),
    scenario: 'human_acceptance',
    origin: 'http://127.0.0.1:17777',
    credential: 'fixture-credential',
    seed: 7,
    tasks,
  });
  assert.equal(manifest.scenario, 'human_acceptance');
  assert.equal(manifest.harness_hash, '1'.repeat(64));
  assert.equal(manifest.seed, 7);
  assert.match(manifest.build_hash, /^[a-f0-9]{64}$/);
  assert.deepEqual(manifest.deep_links.h1, [
    '/tasks/tsk_workbench-running',
    '/tasks/tsk_workbench-needs-attention',
    '/tasks/tsk_workbench-completed',
  ]);
  assert.deepEqual(manifest.deep_links.h2, [
    '/tasks/tsk_context-current',
    '/tasks/tsk_context-historical',
    '/tasks/tsk_context-drifted',
    '/tasks/tsk_context-unavailable',
  ]);
  assert.equal(new Set([...manifest.deep_links.h1, ...manifest.deep_links.h2]).size, 7);
  assert.equal(
    [...manifest.deep_links.h1, ...manifest.deep_links.h2].some((path) => path.includes('?')),
    false,
  );
  assert.equal(manifest.deep_links.h3, undefined);
});

test('rejects loopback listen addresses for a formal real-phone gate', () => {
  assert.throws(() => validateListen('127.0.0.1:0', true), /reachable non-loopback/);
  assert.equal(validateListen('192.168.1.20:0', true), '192.168.1.20:0');
});

test('hashes directory files independent of creation order', async () => {
  const root = await mkdtemp(join(tmpdir(), 'kuku-acceptance-hash-'));
  try {
    await mkdir(join(root, 'nested'));
    await writeFile(join(root, 'z.txt'), 'z');
    await writeFile(join(root, 'nested', 'a.txt'), 'a');
    assert.equal(await hashPath(root), await hashPath(root));
  } finally {
    await rm(root, { force: true, recursive: true });
  }
});

test('hashes every acceptance harness source', async () => {
  const root = await mkdtemp(join(tmpdir(), 'kuku-acceptance-harness-hash-'));
  try {
    const first = join(root, 'first.mjs');
    const second = join(root, 'second.mjs');
    await writeFile(first, 'first');
    await writeFile(second, 'second');
    const before = await hashPaths([second, first]);
    assert.equal(before, await hashPaths([first, second]));
    await writeFile(second, 'changed');
    assert.notEqual(before, await hashPaths([first, second]));
  } finally {
    await rm(root, { force: true, recursive: true });
  }
});

test('uses one canonical acceptance harness identity', async () => {
  assert.equal(
    await hashAcceptanceHarness(),
    await hashPaths([
      join(import.meta.dirname, 'acceptance-human.mjs'),
      join(import.meta.dirname, 'acceptance-provider.mjs'),
    ]),
  );
});

test('builds a link-free candidate identity for automated gates', () => {
  const identity = buildCandidateIdentity({
    sourceSha: 'a'.repeat(40),
    binaryHash: 'b'.repeat(64),
    assetsHash: 'c'.repeat(64),
    contractHash: 'd'.repeat(64),
    baselineHash: 'e'.repeat(64),
    scenarioHash: 'f'.repeat(64),
    harnessHash: '1'.repeat(64),
    scenario: 'human_acceptance',
    seed: 7,
  });
  assert.equal(identity.harness_hash, '1'.repeat(64));
  assert.match(identity.build_hash, /^[a-f0-9]{64}$/);
  assert.equal('deep_links' in identity, false);
});

test('workflows bind every gate to authoritative exact-SHA candidate artifacts', async () => {
  const [a3, human, promotion, a5, release] = await Promise.all(
    ['web-ci.yml', 'webui-human-gates.yml', 'webui-promote.yml', 'webui-rc.yml', 'release.yml'].map(
      (name) => readFile(join(repositoryRoot, '.github/workflows', name), 'utf8'),
    ),
  );

  assert.match(a3, /harnessHash: await hashAcceptanceHarness\(\)/);
  assert.match(
    a3,
    /name: webui-a3-\$\{\{ github\.sha \}\}[\s\S]*path: artifacts\/a3\/manifest\.json/,
  );
  assert.doesNotMatch(human, /manifest_artifact:/);
  assert.match(human, /webui-a3-\$SOURCE_SHA/);
  assert.match(human, /webui-a5-\$SOURCE_SHA/);
  assert.match(human, /hashAcceptanceHarness/);
  assert.doesNotMatch(promotion, /manifest_artifact:/);
  assert.match(promotion, /webui-a3-\$SOURCE_SHA/);
  assert.match(promotion, /a4-rebuild-\$\{\{ inputs\.source_sha \}\}/);
  assert.match(promotion, /harness_hash/);
  assert.match(a5, /harness_hash/);
  assert.match(release, /harness_hash/);
});

test('Human launchers do not activate process-global scenario controls', () => {
  const environment = buildServerEnvironment('h3', 7, {
    KUKU_TEST_SCENARIO: 'inherited',
    KUKU_TEST_SEED: '999',
    PATH: '/usr/bin',
  });
  assert.equal(environment.KUKU_TEST_SCENARIO, undefined);
  assert.equal(environment.KUKU_TEST_SEED, undefined);
  assert.equal(environment.PATH, '/usr/bin');

  const instrumented = buildServerEnvironment('h1-h2', 7, {
    KUKU_TEST_SCENARIO: 'inherited',
    KUKU_TEST_SEED: '999',
  });
  assert.equal(instrumented.KUKU_TEST_SCENARIO, undefined);
  assert.equal(instrumented.KUKU_TEST_SEED, undefined);
});

test('H3 binds the selected A5 archive to the promotion asset identity', () => {
  const sourceSha = 'a'.repeat(40);
  const archiveHash = 'b'.repeat(64);
  const promotion = {
    source_sha: sourceSha,
    assets_hash: 'c'.repeat(64),
    generated_contract_hash: 'd'.repeat(64),
    visual_baseline_hash: 'e'.repeat(64),
    scenario_hash: 'f'.repeat(64),
    harness_hash: '1'.repeat(64),
    scenario: 'human_acceptance',
    seed: 7,
  };
  const identity = resolveH3CandidateIdentity(
    {
      source_sha: sourceSha,
      harness_hash: promotion.harness_hash,
      promotion_manifest: promotion,
      package_sha256sums: [`${archiveHash}  kuku-linux-x86_64.tar.gz`],
    },
    sourceSha,
    archiveHash,
  );
  assert.equal(identity.assetsHash, promotion.assets_hash);
  assert.equal(identity.contractHash, promotion.generated_contract_hash);
  assert.equal(identity.harnessHash, promotion.harness_hash);
  assert.throws(
    () =>
      resolveH3CandidateIdentity(
        {
          source_sha: sourceSha,
          harness_hash: '2'.repeat(64),
          promotion_manifest: promotion,
          package_sha256sums: [`${archiveHash}  kuku-linux-x86_64.tar.gz`],
        },
        sourceSha,
        archiveHash,
      ),
    /harness identity/,
  );
  assert.throws(
    () =>
      resolveH3CandidateIdentity(
        {
          source_sha: sourceSha,
          harness_hash: promotion.harness_hash,
          promotion_manifest: promotion,
          package_sha256sums: [],
        },
        sourceSha,
        archiveHash,
      ),
    /archive hash is not present/,
  );
});
