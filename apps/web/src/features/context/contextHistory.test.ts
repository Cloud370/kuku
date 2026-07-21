import { describe, expect, it } from 'vitest';

import { reduceContextHistory } from './contextHistory';
import { requestOne } from './testFixtures';

describe('reduceContextHistory', () => {
  it('selects historical Context without changing the latest staging target', () => {
    const state = reduceContextHistory(
      { selectedRequestId: null, mode: 'current', stagingTarget: 'latest' },
      { type: 'select', requestId: requestOne },
    );

    expect(state).toEqual({
      selectedRequestId: requestOne,
      mode: 'verified',
      stagingTarget: 'latest',
    });
  });

  it('returns to current Context without changing the latest staging target', () => {
    const state = reduceContextHistory(
      { selectedRequestId: requestOne, mode: 'verified', stagingTarget: 'latest' },
      { type: 'current' },
    );

    expect(state).toEqual({ selectedRequestId: null, mode: 'current', stagingTarget: 'latest' });
  });
});
