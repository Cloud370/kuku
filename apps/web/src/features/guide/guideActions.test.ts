import { describe, expect, it } from 'vitest';

import { GuideAction, guideActionRoute } from './guideActions';

describe('guideActionRoute', () => {
  it('maps typed actions to real feature routes', () => {
    expect(guideActionRoute(GuideAction.Settings)).toBe('/settings');
    expect(guideActionRoute(GuideAction.FirstTask)).toBe('/tasks/new');
    expect(guideActionRoute(GuideAction.ReviewChanges)).toBe('/tasks/new?guide=review');
  });
});
