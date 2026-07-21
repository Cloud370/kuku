import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

import Ajv2020 from 'ajv/dist/2020.js';

const schemaUrl = new URL('../acceptance/evidence.schema.json', import.meta.url);
const schema = JSON.parse(await readFile(schemaUrl, 'utf8'));
const validateSchema = new Ajv2020({ allErrors: true }).compile(schema);

function schemaErrors(evidence) {
  if (validateSchema(evidence)) return [];
  return (validateSchema.errors ?? []).map((error) => {
    const path =
      error.instancePath ||
      (error.keyword === 'required' ? `/${String(error.params.missingProperty)}` : '/');
    return `${path} ${error.message ?? 'is invalid'}`;
  });
}

function checkRunsArray(checkRuns) {
  if (Array.isArray(checkRuns)) return checkRuns;
  return Array.isArray(checkRuns?.check_runs) ? checkRuns.check_runs : [];
}

export function validateHumanEvidence(evidence, candidate, checkRuns) {
  const errors = schemaErrors(evidence);
  if (errors.length > 0) return { accepted: false, errors };

  for (const field of ['source_sha', 'build_hash', 'assets_hash', 'scenario']) {
    if (evidence[field] !== candidate?.[field]) {
      errors.push(`evidence ${field} does not match candidate`);
    }
  }

  const expectedCandidateGate = evidence.gate === 'H3' ? 'h3' : 'h1-h2';
  if (candidate?.gate !== expectedCandidateGate) {
    errors.push(`evidence ${evidence.gate} does not match candidate gate`);
  }

  const predecessor = evidence.gate === 'H3' ? 'A5 Release Candidate' : 'A3 Embedded Chromium';
  const predecessorPassed = checkRunsArray(checkRuns).some(
    (check) =>
      check?.name === predecessor &&
      check?.conclusion === 'success' &&
      check?.head_sha === candidate?.source_sha,
  );
  if (!predecessorPassed) errors.push(`${evidence.gate} requires ${predecessor}`);

  if (evidence.decision === 'accept' && evidence.new_sha !== null) {
    errors.push('accepted evidence must not name a replacement SHA');
  }
  if (
    evidence.decision === 'reject' &&
    (evidence.owner === null || evidence.issue_url === null || evidence.new_sha === null)
  ) {
    errors.push('rejected evidence requires owner, issue_url, and new_sha');
  }

  return { accepted: evidence.decision === 'accept' && errors.length === 0, errors };
}

async function main() {
  const [evidencePath, candidatePath, checkRunsPath] = process.argv.slice(2);
  if (evidencePath === undefined || candidatePath === undefined || checkRunsPath === undefined) {
    throw new Error(
      'usage: validate-human-evidence.mjs <evidence.json> <manifest.json> <check-runs.json>',
    );
  }
  const [evidence, candidate, checkRuns] = await Promise.all(
    [evidencePath, candidatePath, checkRunsPath].map(async (path) =>
      JSON.parse(await readFile(path, 'utf8')),
    ),
  );
  const result = validateHumanEvidence(evidence, candidate, checkRuns);
  if (!result.accepted) {
    for (const error of result.errors) process.stderr.write(`${error}\n`);
    if (result.errors.length === 0) process.stderr.write('human reviewer rejected the candidate\n');
    process.exitCode = 1;
    return;
  }
  process.stdout.write(`${evidence.gate} human evidence accepted\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
