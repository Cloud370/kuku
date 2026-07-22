import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ContextDetail } from './ContextDetail';
import { catalogFixture, contextFixture, requestOne } from './testFixtures';

afterEach(cleanup);

describe('ContextDetail', () => {
  it('renders bounded history honestly and exposes immutable exact content', async () => {
    const user = userEvent.setup();
    const onSelectRequest = vi.fn();
    render(
      <ContextDetail
        catalog={catalogFixture()}
        onOpenAgent={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenSectionsChange={vi.fn()}
        onSelectRequest={onSelectRequest}
        onStageSkill={vi.fn()}
        onUnstageSkill={vi.fn()}
        openSections={['skills']}
        snapshot={contextFixture({ request_history_truncated: true })}
        stagedSkillIds={[]}
        workspaceId="wsp_000000000000000000000001"
      />,
    );

    expect(screen.getByText('Earlier Requests are available from Chat')).toBeVisible();
    await user.click(screen.getByRole('button', { name: `Select Request ${requestOne}` }));
    expect(onSelectRequest).toHaveBeenCalledWith(requestOne);

    const trigger = screen.getByRole('button', { name: 'View exact Request' });
    trigger.focus();
    await user.click(trigger);
    expect(screen.getByRole('dialog', { name: 'Exact Request' })).toBeVisible();
    expect(screen.getByText('System prompt')).toBeVisible();
    expect(screen.getByText('sha256:exact-request')).toBeVisible();
    expect(screen.queryByRole('textbox')).toBeNull();
    await user.keyboard('{Escape}');
    expect(trigger).toHaveFocus();
  });

  it('uses native keyboard accordion controls', async () => {
    const user = userEvent.setup();
    function Harness() {
      const [openSections, setOpenSections] = useState<
        import('./contextSections').ContextSectionKey[]
      >([]);
      return (
        <ContextDetail
          catalog={catalogFixture()}
          onOpenAgent={vi.fn()}
          onOpenFile={vi.fn()}
          onOpenSectionsChange={setOpenSections}
          onSelectRequest={vi.fn()}
          onStageSkill={vi.fn()}
          onUnstageSkill={vi.fn()}
          openSections={openSections}
          snapshot={contextFixture()}
          stagedSkillIds={[]}
          workspaceId="wsp_000000000000000000000001"
        />
      );
    }
    render(<Harness />);

    const skills = screen.getByRole('button', { name: /Skills/ });
    skills.focus();
    await user.keyboard(' ');
    expect(skills).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('Reviews Rust changes safely.')).toBeVisible();
  });
});
