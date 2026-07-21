import Ajv2020, { type ValidateFunction } from 'ajv/dist/2020.js';
import { describe, expect, it } from 'vitest';

import { ContractDecodeError, decodeApiError, decodeTaskStreamEvent } from './decode';
import contractSchema from './generated/schema.json';
import type { ObservationContextItem, SourceFact } from './generated';

const fixtureModules: Record<string, unknown> = import.meta.glob('./generated/fixtures/*.json', {
  eager: true,
  import: 'default',
});

const contractFixtures = Object.entries(fixtureModules)
  .map(([path, value]) => ({ name: path.split('/').at(-1) ?? path, value }))
  .sort((left, right) => left.name.localeCompare(right.name));

const expectedFixtureNames = [
  'api_error.json',
  'context_snapshot.json',
  'platform_catalog.json',
  'platform_status.json',
  'review_snapshot.json',
  'settings_snapshot.json',
  'task_changes.json',
  'task_projection.json',
  'task_stream_event.json',
  'task_stream_event.projection_replaced.json',
  'task_stream_event.timeline_atomic_batch.json',
  'task_stream_event.timeline_single_eviction.json',
];

interface GeneratedSchema {
  $defs: Record<string, object>;
}

const definitions = (contractSchema as GeneratedSchema).$defs;
const ajv = new Ajv2020({
  allErrors: true,
  logger: false,
  strict: false,
  validateFormats: false,
});
const validators = new Map<string, ValidateFunction>();

function definitionValidator(name: string): ValidateFunction {
  let validate = validators.get(name);
  if (validate === undefined) {
    const definition = definitions[name];
    if (definition === undefined) throw new Error(`Missing generated definition ${name}`);
    validate = ajv.compile({ ...definition, $defs: definitions });
    validators.set(name, validate);
  }
  return validate;
}

function expectDefinition(name: string, value: unknown): void {
  const validate = definitionValidator(name);
  expect(validate(value), JSON.stringify(validate.errors)).toBe(true);
}

function expectDefinitionRejected(name: string, value: unknown): void {
  const validate = definitionValidator(name);
  expect(validate(value), `${name} accepted ${JSON.stringify(value)}`).toBe(false);
}

function expectFixtureValid(name: string, value: unknown): void {
  if (name === 'task_changes.json') {
    const bundle = value as { changes: unknown[]; deltas: unknown[] };
    bundle.changes.forEach((change) => {
      expectDefinition('TaskChange', change);
    });
    bundle.deltas.forEach((delta) => {
      expectDefinition('TaskDelta', delta);
    });
    return;
  }
  if (name.startsWith('task_stream_event')) {
    expect(decodeTaskStreamEvent(value)).toEqual(value);
    return;
  }
  if (name === 'api_error.json') {
    expect(decodeApiError(value)).toEqual(value);
    return;
  }

  const definitionByFixture: Record<string, string> = {
    'context_snapshot.json': 'ContextSnapshot',
    'platform_catalog.json': 'PlatformCatalog',
    'platform_status.json': 'PlatformStatus',
    'review_snapshot.json': 'ReviewSnapshot',
    'settings_snapshot.json': 'SettingsSnapshot',
    'task_projection.json': 'TaskProjection',
  };
  const definition = definitionByFixture[name];
  if (definition === undefined) throw new Error(`No validator registered for fixture ${name}`);
  expectDefinition(definition, value);
}

function validStreamEvent(): Record<string, unknown> {
  return {
    api_version: 1,
    cursor: 8,
    task_revision: 2,
    task_id: 'tsk_000000000000000000000001',
    event: {
      type: 'changes_applied',
      changes: [],
      timeline_window: null,
    },
  };
}

function validApiError(): Record<string, unknown> {
  return {
    api_version: 1,
    code: 'task_busy',
    message: 'Task already has an active run',
    trace_id: 'trace_fixture',
    details: null,
  };
}

describe('decodeTaskStreamEvent', () => {
  it('returns a schema-valid stream event', () => {
    const value = validStreamEvent();

    expect(decodeTaskStreamEvent(value)).toEqual(value);
  });

  it('rejects an unsupported API version', () => {
    const value = validStreamEvent();
    value.api_version = 2;

    expect(() => decodeTaskStreamEvent(value)).toThrow(ContractDecodeError);
  });

  it('rejects string and unsafe integer cursors', () => {
    const stringCursor = validStreamEvent();
    stringCursor.cursor = '7';
    const unsafeCursor = validStreamEvent();
    unsafeCursor.cursor = 9_007_199_254_740_992;

    expect(() => decodeTaskStreamEvent(stringCursor)).toThrow(ContractDecodeError);
    expect(() => decodeTaskStreamEvent(unsafeCursor)).toThrow(ContractDecodeError);
  });

  it('rejects an unknown outer delta kind', () => {
    const value = validStreamEvent();
    value.event = { type: 'record_replayed' };

    expect(() => decodeTaskStreamEvent(value)).toThrow(ContractDecodeError);
  });

  it('rejects an unknown inner change kind', () => {
    const value = validStreamEvent();
    value.event = {
      type: 'changes_applied',
      changes: [{ type: 'message_replaced' }],
      timeline_window: null,
    };

    expect(() => decodeTaskStreamEvent(value)).toThrow(ContractDecodeError);
  });

  it('rejects a missing required nullable timeline window', () => {
    const value = validStreamEvent();
    value.event = { type: 'changes_applied', changes: [] };

    expect(() => decodeTaskStreamEvent(value)).toThrow(ContractDecodeError);
  });

  it('rejects a malformed timeline window', () => {
    const value = validStreamEvent();
    value.event = {
      type: 'changes_applied',
      changes: [],
      timeline_window: { next_cursor: null, evicted_items: 'message-000000' },
    };

    expect(() => decodeTaskStreamEvent(value)).toThrow(ContractDecodeError);
  });
});

describe('Rust contract fixtures', () => {
  it('keeps workspace-relative paths as structural strings', () => {
    const source: SourceFact = {
      id: 'source:fixture',
      relative_path: 'src/lib.rs',
      scope: 'workspace',
    };
    const observation: ObservationContextItem = {
      current_drift: 'present',
      kind: { kind: 'file_read' },
      relative_path: 'src/lib.rs',
      request_id: 'req_000000000000000000000001',
      retention: 'retained',
      summary: 'Read the module entrypoint',
      tool_call_id: 'call_read_file',
    };

    expect(source.relative_path).toBe('src/lib.rs');
    expect(observation.relative_path).toBe('src/lib.rs');
    for (const relativePath of ['src/lib.rs', '.git/config', '路径/文件', 'console.txt']) {
      expectDefinition('SourceFact', { ...source, relative_path: relativePath });
    }
    for (const relativePath of [
      '/etc/passwd',
      '../secret',
      'src/../secret',
      'src\\lib.rs',
      'report.txt:secret',
      'CON',
      'COM¹',
      'NUL...log',
      'nested/PrN.log',
      'file.',
      'nested /file',
    ]) {
      expectDefinitionRejected('SourceFact', { ...source, relative_path: relativePath });
    }
  });

  it('exposes every exported fixture to browser validation', () => {
    expect(contractFixtures.map(({ name }) => name)).toEqual(expectedFixtureNames);
  });

  it.each(contractFixtures)('decodes Rust fixture $name', ({ name, value }) => {
    expectFixtureValid(name, value);
  });
});

describe('decodeApiError', () => {
  it('returns a schema-valid typed API error', () => {
    const value = validApiError();

    expect(decodeApiError(value)).toEqual(value);
  });

  it('rejects an unknown API error code', () => {
    const value = validApiError();
    value.code = 'unknown_code';

    expect(() => decodeApiError(value)).toThrow(ContractDecodeError);
  });

  it('rejects a missing required nullable details key', () => {
    const value = validApiError();
    delete value.details;

    expect(() => decodeApiError(value)).toThrow(ContractDecodeError);
  });
});
