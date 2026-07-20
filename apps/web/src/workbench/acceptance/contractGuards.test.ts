import { readFileSync, readdirSync } from 'node:fs';
import { extname, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const workbenchRoot = resolve(process.cwd(), 'src/workbench');

function sourceFiles(root: string): string[] {
  return readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(root, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    if (!['.ts', '.tsx'].includes(extname(path)) || path.includes('.test.')) return [];
    return [readFileSync(path, 'utf8')];
  });
}

describe('Workbench contract guards', () => {
  it('uses no Session, raw replay, or legacy command vocabulary', () => {
    const shippedSource = sourceFiles(workbenchRoot).join('\n');
    for (const token of ['/sessions', 'replayToTurns', 'StoredEventItem', 'TerminalPanel']) {
      expect(shippedSource).not.toContain(token);
    }
  });
});
