import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { SkillLoadOrigin } from '../../api/generated';
import { ContextSectionContent } from './ContextSectionContent';
import { selectContextView } from './contextSelectors';
import { catalogFixture, contextFixture, workspaceId } from './testFixtures';

afterEach(cleanup);

describe('ContextSectionContent', () => {
  it.each([
    ['agent', 'Loaded by Agent'],
    ['you', 'Loaded by You'],
    ['bootstrap', 'Loaded by Bootstrap'],
    ['project', 'Loaded by Project'],
  ] satisfies Array<[SkillLoadOrigin, string]>)('labels %s Skill truth as %s', (origin, label) => {
    const fixture = contextFixture();
    const skill = fixture.sections.skills[0];
    if (skill === undefined) throw new Error('Skill fixture is missing');
    const view = selectContextView(
      contextFixture({
        sections: {
          ...fixture.sections,
          skills: [{ ...skill, origin }],
        },
      }),
      catalogFixture(),
      [],
    );

    render(
      <ContextSectionContent
        onOpenAgent={vi.fn()}
        onOpenFile={vi.fn()}
        onStageSkill={vi.fn()}
        onUnstageSkill={vi.fn()}
        section="skills"
        view={view}
        workspaceId={workspaceId}
      />,
    );

    expect(screen.getByText(label)).toBeVisible();
    if (origin !== 'agent') expect(screen.queryByText('Loaded by Agent')).toBeNull();
  });
});
