import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ContextDetail } from './ContextDetail';
import { catalogFixture, contextFixture, requestOne } from './testFixtures';

afterEach(cleanup);

describe('ContextDetail', () => {
  function requestHistory(count: number) {
    const template = contextFixture().request_history[0];
    if (template === undefined) throw new Error('request fixture is missing');
    return Array.from({ length: count }, (_, index) => ({
      ...template,
      request_id: `req_${String(index + 1).padStart(24, '0')}`,
      model: `model-${String(index + 1)}`,
      started_at: `2026-07-21T00:${String(index + 1).padStart(2, '0')}:00Z`,
    }));
  }

  function requestIdAt(requests: ReturnType<typeof requestHistory>, index: number) {
    const request = requests[index];
    if (request === undefined) throw new Error(`request ${String(index)} is missing`);
    return request.request_id;
  }

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

  it('shows the five latest Requests first and expands the complete bounded history', async () => {
    const user = userEvent.setup();
    const requests = requestHistory(8);
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
        snapshot={contextFixture({
          request_history: requests,
          selected_request: requests[7],
        })}
        stagedSkillIds={[]}
        workspaceId="wsp_000000000000000000000001"
      />,
    );

    expect(
      screen.queryByRole('button', { name: `Select Request ${requestIdAt(requests, 2)}` }),
    ).toBeNull();
    expect(
      screen.getByRole('button', { name: `Select Request ${requestIdAt(requests, 7)}` }),
    ).toBeVisible();
    expect(screen.getByText('8 Requests')).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Show all 8 Requests' }));
    expect(
      screen.getByRole('button', { name: `Select Request ${requestIdAt(requests, 0)}` }),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: 'Show recent 5 Requests' })).toBeVisible();
  });

  it('keeps an older selected Request visible while history is collapsed', () => {
    const requests = requestHistory(8);
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
        snapshot={contextFixture({
          request_history: requests,
          selected_request: requests[0],
        })}
        stagedSkillIds={[]}
        workspaceId="wsp_000000000000000000000001"
      />,
    );

    expect(
      screen.getByRole('button', { name: `Select Request ${requestIdAt(requests, 0)}` }),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: `Select Request ${requestIdAt(requests, 7)}` }),
    ).toBeVisible();
    expect(
      screen.queryByRole('button', { name: `Select Request ${requestIdAt(requests, 3)}` }),
    ).toBeNull();
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
