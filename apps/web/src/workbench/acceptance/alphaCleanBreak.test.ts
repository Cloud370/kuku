import { readFileSync, readdirSync } from 'node:fs';
import { extname, resolve } from 'node:path';
import { expect, it } from 'vitest';

const workbenchRoot = resolve(process.cwd(), 'src/workbench');

function workbenchSource(root: string): string {
  return readdirSync(root, { withFileTypes: true })
    .flatMap((entry) => {
      const path = resolve(root, entry.name);
      if (entry.isDirectory()) return [workbenchSource(path)];
      if (!['.ts', '.tsx'].includes(extname(path)) || path.includes('.test.')) return [];
      return [readFileSync(path, 'utf8')];
    })
    .join('\n');
}

it('keeps the production Workbench independent from the discarded demo frontend', () => {
  const source = workbenchSource(workbenchRoot);
  for (const token of [
    'routes/Home',
    'routes/Session',
    'components/chat',
    'components/layout',
    'components/right-panel',
    'stores/ui',
  ]) {
    expect(source).not.toContain(token);
  }
});
