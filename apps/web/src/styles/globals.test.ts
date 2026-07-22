import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

describe('global scrollbars', () => {
  it('uses a thin themed scrollbar with a transparent track', () => {
    const source = readFileSync(join(process.cwd(), 'src', 'styles', 'globals.css'), 'utf8');

    expect(source).toContain('scrollbar-width: thin');
    expect(source).toContain('scrollbar-color: var(--scrollbar-thumb) transparent');
    expect(source).toContain('*::-webkit-scrollbar');
    expect(source).toContain('*::-webkit-scrollbar-track');
    expect(source).toContain('*::-webkit-scrollbar-thumb');
  });
});
