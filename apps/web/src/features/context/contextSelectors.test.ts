import { describe, expect, it } from 'vitest';

import { catalogFixture, contextFixture } from './testFixtures';
import {
  defaultOpenSections,
  isWorkspaceRelativePath,
  selectContextView,
} from './contextSelectors';

describe('selectContextView', () => {
  it('keeps every authoritative section and both usage scopes', () => {
    const fixture = contextFixture();
    const view = selectContextView(fixture, catalogFixture(), []);

    expect(view.selectedRequest?.request_id).toBe('req_000000000000000000000002');
    expect(view.requestHistory.map((request) => request.request_id)).toEqual([
      'req_000000000000000000000001',
      'req_000000000000000000000002',
    ]);
    expect(Object.keys(view.sections)).toEqual([
      'skills',
      'instructions',
      'memory',
      'conversation',
      'observations',
      'agents',
      'capabilities',
    ]);
    expect(view.usage.thisRequest).toEqual(fixture.usage.this_request);
    expect(view.usage.thisTask).toEqual(fixture.usage.this_task);
    expect(view.warnings).toEqual(fixture.warnings);
    expect(view.exactRequest).toEqual(fixture.exact_request);
    expect(view.exactPayloadHash).toBe('sha256:exact-request');
    expect(view.sections.observations.map((observation) => observation.current_drift)).toEqual([
      'changed_since_observation',
      'inaccessible',
    ]);
  });

  it('never upgrades discoverable Skills to loaded', () => {
    const fixture = contextFixture({ sections: { ...contextFixture().sections, skills: [] } });
    const view = selectContextView(fixture, catalogFixture(), []);

    expect(view.discoverable.skills[0]?.truth).toBe('Discoverable');
    expect(view.sections.skills).toHaveLength(0);
  });

  it('overlays staged Skills only on the next Request preview', () => {
    const fixture = contextFixture();
    const view = selectContextView(fixture, catalogFixture(), ['skill:project:docs']);

    expect(view.staged.map((skill) => skill.skill_id)).toEqual(['skill:project:docs']);
    expect(view.sections.skills).toEqual(fixture.sections.skills);
    expect(view.nextRequestBase).toEqual(fixture.next_request_base);
  });

  it('defaults open Staged, Skills, and warning-bearing sections', () => {
    expect(defaultOpenSections(contextFixture(), ['skill:project:docs'])).toEqual([
      'staged',
      'skills',
      'observations',
      'health',
    ]);
    expect(
      defaultOpenSections(
        contextFixture({
          warnings: [],
          health: {
            ...contextFixture().health,
            source_drift_count: 0,
            truncated_observation_count: 0,
          },
        }),
        [],
      ),
    ).toEqual(['skills']);
  });
});

describe('isWorkspaceRelativePath', () => {
  it('accepts long canonical paths and rejects absolute or escaping paths', () => {
    expect(isWorkspaceRelativePath(`${'deep/'.repeat(80)}file.rs`)).toBe(true);
    expect(isWorkspaceRelativePath('/etc/passwd')).toBe(false);
    expect(isWorkspaceRelativePath('C:\\Windows\\system.ini')).toBe(false);
    expect(isWorkspaceRelativePath('../secret')).toBe(false);
    expect(isWorkspaceRelativePath('src//lib.rs')).toBe(false);
  });
});
