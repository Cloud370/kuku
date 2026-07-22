import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { webApi } from '../../api/client';
import { ContextPanel } from './ContextPanel';
import { resolveInitialOpenSections } from './contextSections';
import { catalogFixture, contextFixture, requestOne, taskId, workspaceId } from './testFixtures';

const callbacks = {
  onSelectRequest: vi.fn(),
  onOpenAgent: vi.fn(),
  onOpenFile: vi.fn(),
  onStageSkill: vi.fn(),
  onUnstageSkill: vi.fn(),
  onOpenSectionsChange: vi.fn(),
};

function renderPanel(
  stagedSkillIds: string[] = [],
  selectedRequestId: string | null = null,
  refreshRevision = 0,
) {
  return render(
    <ContextPanel
      {...callbacks}
      openSections={['skills', 'observations', 'discoverable']}
      refreshRevision={refreshRevision}
      selectedRequestId={selectedRequestId}
      stagedSkillIds={stagedSkillIds}
      taskId={taskId}
      workspaceId={workspaceId}
    />,
  );
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  Object.values(callbacks).forEach((callback) => callback.mockReset());
});

describe('ContextPanel', () => {
  it('opens all useful sections by default while preserving saved choices', () => {
    expect(resolveInitialOpenSections([])).toEqual([
      'staged',
      'skills',
      'instructions',
      'memory',
      'conversation',
      'observations',
      'agents',
      'discoverable',
      'capabilities',
      'usage',
      'health',
    ]);
    expect(resolveInitialOpenSections(['usage', 'health', 'invalid'])).toEqual([
      'usage',
      'health',
    ]);
  });

  it('loads current and historical snapshots through the canonical client', async () => {
    const historicalRequest = contextFixture().request_history.at(0);
    if (historicalRequest === undefined) throw new Error('historical fixture is missing');
    vi.spyOn(webApi.context, 'current').mockResolvedValue(contextFixture());
    vi.spyOn(webApi.context, 'historical').mockResolvedValue(
      contextFixture({
        selected_request: {
          ...historicalRequest,
          request_id: requestOne,
        },
      }),
    );
    vi.spyOn(webApi.catalog, 'workspace').mockResolvedValue(catalogFixture());

    const current = renderPanel();
    expect(screen.getByRole('status')).toHaveTextContent('Loading Context');
    expect(await screen.findByText('2,048 tokens')).toBeVisible();
    expect(screen.getByLabelText('Context usage')).toHaveTextContent('25%');
    expect(screen.getByLabelText('Cache hit rate')).toHaveTextContent('25%');
    expect(screen.getByLabelText('Task request count')).toHaveTextContent('4');
    expect(webApi.context.current).toHaveBeenCalledWith(taskId);
    current.unmount();

    renderPanel([], requestOne);
    expect(await screen.findByText(/Historical Request/)).toBeVisible();
    expect(webApi.context.historical).toHaveBeenCalledWith(taskId, requestOne);
  });

  it('shows Staged only while staged Skill IDs are present', async () => {
    vi.spyOn(webApi.context, 'current').mockResolvedValue(contextFixture());
    vi.spyOn(webApi.catalog, 'workspace').mockResolvedValue(catalogFixture());
    const view = renderPanel();
    await screen.findByText('2,048 tokens');
    expect(screen.queryByRole('button', { name: /Staged/ })).toBeNull();

    view.rerender(
      <ContextPanel
        {...callbacks}
        openSections={['staged', 'skills']}
        selectedRequestId={null}
        stagedSkillIds={['skill:project:docs']}
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );
    expect(await screen.findByRole('button', { name: /Staged/ })).toBeVisible();
    expect(webApi.context.current).toHaveBeenCalledOnce();
    expect(screen.getByRole('status')).toHaveTextContent('Context loaded');
    await userEvent.click(screen.getByRole('button', { name: /Remove Documentation guide/ }));
    expect(callbacks.onUnstageSkill).toHaveBeenCalledWith('skill:project:docs');
  });

  it('reloads current Context after a relevant committed task delta', async () => {
    vi.spyOn(webApi.context, 'current').mockResolvedValue(contextFixture());
    vi.spyOn(webApi.catalog, 'workspace').mockResolvedValue(catalogFixture());
    const view = renderPanel();
    await screen.findByText('2,048 tokens');

    view.rerender(
      <ContextPanel
        {...callbacks}
        openSections={['skills']}
        refreshRevision={1}
        selectedRequestId={null}
        stagedSkillIds={[]}
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    await waitFor(() => {
      expect(webApi.context.current).toHaveBeenCalledTimes(2);
    });
  });

  it('uses the fixed empty state without an accordion or phantom Staged section', async () => {
    vi.spyOn(webApi.context, 'current').mockResolvedValue(
      contextFixture({ selected_request: null, request_history: [] }),
    );
    vi.spyOn(webApi.catalog, 'workspace').mockResolvedValue(catalogFixture());
    renderPanel();

    expect(
      await screen.findByText('Context will appear after the first provider Request.'),
    ).toBeVisible();
    expect(screen.queryByLabelText('Context sections')).toBeNull();
    expect(screen.queryByRole('button', { name: /Staged/ })).toBeNull();
  });

  it('omits unavailable Request metrics instead of rendering zero values', async () => {
    const fixture = contextFixture({
      usage: { ...contextFixture().usage, this_request: null },
      health: {
        ...contextFixture().health,
        level: 'unavailable',
        context_tokens_used: null,
      },
    });
    vi.spyOn(webApi.context, 'current').mockResolvedValue(fixture);
    vi.spyOn(webApi.catalog, 'workspace').mockResolvedValue(catalogFixture());
    render(
      <ContextPanel
        {...callbacks}
        openSections={['usage']}
        selectedRequestId={null}
        stagedSkillIds={[]}
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    expect(await screen.findByText('Token usage unavailable')).toBeVisible();
    expect(screen.getByText('Usage unavailable.')).toBeVisible();
    expect(within(screen.getByLabelText('This Request')).queryByText('Input')).toBeNull();
  });

  it('opens only canonical relative observation paths', async () => {
    vi.spyOn(webApi.context, 'current').mockResolvedValue(contextFixture());
    vi.spyOn(webApi.catalog, 'workspace').mockResolvedValue(catalogFixture());
    const user = userEvent.setup();
    renderPanel();

    const relative = await screen.findByRole('button', { name: 'Open src/lib.rs' });
    await user.click(relative);
    expect(callbacks.onOpenFile).toHaveBeenCalledWith(workspaceId, 'src/lib.rs');
    expect(screen.getByText('/etc/passwd')).toHaveAttribute('aria-disabled', 'true');
  });

  it('renders canonical errors and makes non-repeating polite announcements', async () => {
    vi.spyOn(webApi.context, 'current').mockRejectedValue(
      Object.assign(new Error('Request not found'), { code: 'request_not_found' }),
    );
    vi.spyOn(webApi.catalog, 'workspace').mockResolvedValue(catalogFixture());
    renderPanel();

    expect(await screen.findByText(/request_not_found/)).toBeVisible();
    const live = screen.getByRole('status');
    expect(live).toHaveTextContent('Context unavailable');
    await waitFor(() => {
      expect(screen.getAllByRole('status')).toHaveLength(1);
    });
  });
});
