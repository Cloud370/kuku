import { describe, expect, it } from 'vitest';

import { H2_EVIDENCE } from './h2Evidence';

describe('H2 evidence manifest', () => {
  it('provides deterministic Context evidence targets without recording an approval', () => {
    expect(H2_EVIDENCE.map((entry) => entry.id)).toEqual([
      'context-current-verified',
      'context-history-exact',
      'context-discoverable',
      'context-agent-thread',
      'context-sticky-staged-desktop',
      'context-sticky-staged-360',
      'context-observation-file-return',
      'context-webkit-360',
      'context-zoom-200',
      'context-forced-colors',
    ]);
    expect(H2_EVIDENCE.every((entry) => entry.route.startsWith('/tasks/'))).toBe(true);
    expect(H2_EVIDENCE.every((entry) => entry.expected.length > 0)).toBe(true);
  });
});
