import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
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

  it('uses one Context scroll region with a compact status before task-relevant sections', () => {
    render(
      <ContextDetail
        catalog={catalogFixture()}
        onOpenAgent={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenSectionsChange={vi.fn()}
        onSelectRequest={vi.fn()}
        onStageSkill={vi.fn()}
        onUnstageSkill={vi.fn()}
        openSections={[]}
        snapshot={contextFixture()}
        stagedSkillIds={[]}
        workspaceId="wsp_000000000000000000000001"
      />,
    );

    expect(screen.getAllByRole('region', { name: 'Context details' })).toHaveLength(1);
    expect(screen.getByLabelText('Context status')).toHaveTextContent('Needs attention');

    const labels = within(screen.getByLabelText('Context sections'))
      .getAllByRole('button', { expanded: false })
      .map((button) => button.textContent.replace(/\s+/g, ' ').trim());
    expect(labels.slice(0, 6)).toEqual([
      'Skills1',
      'Instructions1',
      'Workspace observationsAttention',
      'Conversation',
      'Agents1',
      'Memory1',
    ]);
  });

  it('renders scannable Request summaries and reveals technical details on demand', async () => {
    const user = userEvent.setup();
    render(
      <ContextDetail
        catalog={catalogFixture()}
        onOpenAgent={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenSectionsChange={vi.fn()}
        onSelectRequest={vi.fn()}
        onStageSkill={vi.fn()}
        onUnstageSkill={vi.fn()}
        openSections={[]}
        snapshot={contextFixture()}
        stagedSkillIds={[]}
        workspaceId="wsp_000000000000000000000001"
      />,
    );

    const firstRequest = screen.getByRole('button', { name: `Select Request ${requestOne}` });
    expect(firstRequest).toHaveTextContent('Request 1');
    expect(firstRequest).toHaveTextContent('Completed');
    expect(firstRequest).toHaveTextContent('claude-fixture');
    expect(firstRequest).toHaveTextContent('Jul 21');
    expect(screen.queryByText(requestOne)).toBeNull();

    await user.click(screen.getByRole('button', { name: 'Show Request 1 details' }));
    expect(screen.getByText(requestOne)).toBeVisible();
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
