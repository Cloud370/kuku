import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { extname, relative, resolve } from 'node:path';
import { expect, it } from 'vitest';

const srcRoot = resolve(process.cwd(), 'src');
const removedModulePaths = [
  'routes/Home.tsx',
  'routes/Session.tsx',
  'adapters/replay.ts',
  'adapters/replay.test.ts',
  'adapters/stream.ts',
  'api/sessions.ts',
  'api/runs.ts',
  'api/responses.ts',
  'queries/health.ts',
  'queries/health.test.ts',
  'queries/sessions.ts',
  'stores/run.ts',
  'stores/ui.ts',
  'stores/ui.test.ts',
  'types/turn.ts',
  'components/ConnectionGate.tsx',
  'components/ConnectionGate.stories.tsx',
  'components/chat',
  'components/layout',
  'components/right-panel',
] as const;

function shippedSource(root: string): string {
  return readdirSync(root, { withFileTypes: true })
    .flatMap((entry) => {
      const path = resolve(root, entry.name);
      const name = relative(srcRoot, path);
      if (name.startsWith('api/generated') || path.includes('.test.')) return [];
      if (entry.isDirectory()) return [shippedSource(path)];
      if (!['.ts', '.tsx'].includes(extname(path))) return [];
      return [readFileSync(path, 'utf8')];
    })
    .join('\n');
}

it('contains no discarded alpha frontend modules or vocabulary', () => {
  for (const path of removedModulePaths) {
    expect(existsSync(resolve(srcRoot, path)), path).toBe(false);
  }
  const source = shippedSource(srcRoot);
  for (const token of [
    '/sessions',
    'replayToTurns',
    'StoredEventItem',
    'SessionList',
    'TerminalPanel',
  ]) {
    expect(source).not.toContain(token);
  }
});
