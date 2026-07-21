import assert from 'node:assert/strict';
import test from 'node:test';

import { validateHumanEvidence } from './validate-human-evidence.mjs';

const sha = 'a'.repeat(40);
const hash = 'b'.repeat(64);

function manifest(overrides = {}) {
  return {
    gate: 'h1-h2',
    source_sha: sha,
    build_hash: hash,
    assets_hash: 'c'.repeat(64),
    scenario: 'human_acceptance',
    ...overrides,
  };
}

function evidence(overrides = {}) {
  return {
    gate: 'H1',
    source_sha: sha,
    build_hash: hash,
    assets_hash: 'c'.repeat(64),
    scenario: 'human_acceptance',
    browser: 'Chromium',
    device: 'Desktop',
    viewport: '1440x900',
    reviewer: 'human-reviewer',
    reviewed_at: '2026-07-21T12:00:00Z',
    decision: 'accept',
    observations: ['Workbench hierarchy is clear.'],
    evidence_url: 'https://example.invalid/evidence/1',
    owner: null,
    issue_url: null,
    new_sha: null,
    ...overrides,
  };
}

function checks(names = ['A3 Embedded Chromium']) {
  return { check_runs: names.map((name) => ({ name, conclusion: 'success', head_sha: sha })) };
}

test('accepts fresh human evidence with the exact automated predecessor', () => {
  assert.deepEqual(validateHumanEvidence(evidence(), manifest(), checks()).errors, []);
});

test('rejects evidence from another candidate', () => {
  const result = validateHumanEvidence(
    evidence({ build_hash: 'd'.repeat(64) }),
    manifest(),
    checks(),
  );
  assert.deepEqual(result.errors, ['evidence build_hash does not match candidate']);
});

test('requires A5 before H3', () => {
  const result = validateHumanEvidence(
    evidence({ gate: 'H3' }),
    manifest({ gate: 'h3' }),
    checks([]),
  );
  assert.deepEqual(result.errors, ['H3 requires A5 Release Candidate']);
});

test('requires rejection routing and returns a failing decision', () => {
  const result = validateHumanEvidence(
    evidence({ decision: 'reject', owner: null, issue_url: null, new_sha: null }),
    manifest(),
    checks(),
  );
  assert.ok(result.errors.includes('rejected evidence requires owner, issue_url, and new_sha'));
  assert.equal(result.accepted, false);
});

test('rejects a replacement SHA and missing schema fields', () => {
  const stale = validateHumanEvidence(evidence({ new_sha: 'e'.repeat(40) }), manifest(), checks());
  assert.ok(stale.errors.includes('accepted evidence must not name a replacement SHA'));
  const incomplete = evidence();
  delete incomplete.reviewer;
  assert.ok(validateHumanEvidence(incomplete, manifest(), checks()).errors[0]?.startsWith('/'));
});
