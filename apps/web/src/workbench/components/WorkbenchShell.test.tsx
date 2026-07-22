import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import platformStatusJson from '../../api/generated/fixtures/platform_status.json';
import taskProjectionJson from '../../api/generated/fixtures/task_projection.json';
import type { PlatformStatus, TaskProjection, WorkspaceSummary } from '../../api/generated';
import { createWorkbenchSnapshot, type WorkbenchSnapshot } from '../state';

import { WorkbenchHeader } from './WorkbenchHeader';
import { WorkbenchShell, type WorkbenchShellProps } from './WorkbenchShell';

function setViewport(width: number, height = 900) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: width });
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: height });
  window.matchMedia = vi.fn().mockImplementation((query: string) => {
    const maxWidth = /max-width:\s*(\d+)px/.exec(query)?.[1];
    const matches = maxWidth === undefined ? false : width <= Number(maxWidth);
    return {
      addEventListener: vi.fn(),
      addListener: vi.fn(),
      dispatchEvent: vi.fn(),
      matches,
      media: query,
      onchange: null,
      removeEventListener: vi.fn(),
      removeListener: vi.fn(),
    } satisfies MediaQueryList;
  });
}

class TestResizeObserver implements ResizeObserver {
  disconnect = vi.fn();
  observe = vi.fn();
  unobserve = vi.fn();
}

function projection(): TaskProjection {
  return structuredClone(taskProjectionJson) as TaskProjection;
}

function snapshot(taskProjection = projection()): WorkbenchSnapshot {
  return {
    ...createWorkbenchSnapshot(),
    connection: 'ready',
    cursor: taskProjection.cursor,
    projection: taskProjection,
    selectedTaskId: taskProjection.task.task_id,
  };
}

function workspace(branch: string | null = 'feature/search'): WorkspaceSummary {
  return {
    availability: 'available',
    branch,
    is_default: true,
    label: 'kuku',
    workspace_id: 'ws_000000000000000000000001',
  };
}

function shellProps(overrides: Partial<WorkbenchShellProps> = {}): WorkbenchShellProps {
  return {
    chat: <p>Conversation content</p>,
    context: <p>Context content</p>,
    onOpenContext: vi.fn(),
    onStop: vi.fn(),
    platformStatus: structuredClone(platformStatusJson) as PlatformStatus,
    stagedSkillCount: 2,
    state: snapshot(),
    taskNavigation: <button type="button">Task alpha</button>,
    workspace: workspace(),
    ...overrides,
  };
}

function renderAt(width: number, props: Partial<WorkbenchShellProps> = {}) {
  setViewport(width);
  return render(<WorkbenchShell {...shellProps(props)} />);
}

afterEach(() => {
  cleanup();
});

describe('WorkbenchShell', () => {
  beforeEach(() => {
    window.ResizeObserver = TestResizeObserver;
    window.localStorage.clear();
    setViewport(1440);
  });

  it('exposes persistent resize handles between all three desktop columns', () => {
    renderAt(1440);

    expect(screen.getByRole('separator', { name: 'Resize Tasks and Chat' })).toBeVisible();
    expect(screen.getByRole('separator', { name: 'Resize Chat and Agent Context' })).toBeVisible();
  });

  it('delegates scrolling to one content owner in each desktop sidebar', () => {
    renderAt(1440, {
      context: <div data-testid="context-scroll-owner">Context content</div>,
      taskNavigation: <div data-testid="tasks-scroll-owner">Task content</div>,
    });

    const tasksSlot = screen.getByTestId('tasks-scroll-owner').parentElement;
    const contextSlot = screen.getByTestId('context-scroll-owner').parentElement;
    expect(tasksSlot).toHaveClass('overflow-hidden');
    expect(tasksSlot).not.toHaveClass('overflow-y-auto');
    expect(contextSlot).toHaveClass('overflow-hidden');
  });

  it.each([768, 1440])(
    'renders desktop landmarks without a nested Chat surface at %ipx',
    (width) => {
      renderAt(width);

      expect(screen.getByRole('banner')).toBeVisible();
      expect(screen.getByRole('navigation', { name: 'Tasks' })).toBeVisible();
      expect(screen.getByRole('main', { name: 'Chat' })).toBeVisible();
      expect(screen.getByRole('complementary', { name: 'Agent Context' })).toBeVisible();
      expect(screen.getAllByRole('main')).toHaveLength(1);
      expect(screen.getByTestId('workbench-shell')).toHaveClass('min-w-0');
    },
  );

  it('uses a Tasks drawer at 360px and keeps Chat primary', async () => {
    const user = userEvent.setup();
    renderAt(360);

    expect(screen.getByRole('main', { name: 'Chat' })).toBeVisible();
    const trigger = screen.getByRole('button', { name: 'Open Tasks' });
    expect(trigger).toBeVisible();
    expect(screen.queryByRole('navigation', { name: 'Tasks' })).toBeNull();

    await user.click(trigger);
    expect(screen.getByRole('dialog', { name: 'Tasks' })).toBeVisible();
    const taskNavigation = screen.getByRole('navigation', { name: 'Tasks' });
    expect(taskNavigation).toBeVisible();
    expect(taskNavigation).toHaveClass('overflow-hidden');
    expect(taskNavigation).not.toHaveClass('overflow-y-auto');
    expect(screen.getByRole('button', { name: 'Close Tasks' })).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Tasks' })).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it('collapses either desktop sidebar without hiding Chat', async () => {
    const user = userEvent.setup();
    renderAt(1440);

    await user.click(screen.getByRole('button', { name: 'Collapse Tasks' }));
    await user.click(screen.getByRole('button', { name: 'Collapse Agent Context' }));

    expect(screen.getByRole('main', { name: 'Chat' })).toBeVisible();
    expect(screen.queryByRole('navigation', { name: 'Tasks' })).toBeNull();
    expect(screen.queryByRole('complementary', { name: 'Agent Context' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Open Tasks' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Open Agent Context' })).toBeVisible();
  });

  it('opens mobile Agent Context in the canonical drawer and updates the integration route', async () => {
    const user = userEvent.setup();
    const onOpenContext = vi.fn();
    renderAt(360, { onOpenContext });

    await user.click(screen.getByRole('button', { name: 'Open Agent Context' }));

    expect(onOpenContext).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('dialog', { name: 'Agent Context' })).toBeVisible();
    const context = screen.getByRole('complementary', { name: 'Agent Context' });
    expect(context).toHaveTextContent('Context content');
    expect(context).toHaveClass('overflow-hidden');
    expect(context).not.toHaveClass('overflow-y-auto');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Agent Context' })).toBeNull();
  });

  it('shows only the server-supplied current branch', () => {
    const props = shellProps();
    const { rerender } = render(
      <WorkbenchHeader
        onOpenContext={props.onOpenContext}
        onOpenTasks={vi.fn()}
        onStop={props.onStop}
        platformStatus={props.platformStatus}
        stagedSkillCount={props.stagedSkillCount}
        state={props.state}
        workspace={workspace('feature/search')}
      />,
    );
    expect(screen.getByTestId('workspace-branch')).toHaveTextContent('feature/search');

    rerender(
      <WorkbenchHeader
        onOpenContext={props.onOpenContext}
        onOpenTasks={vi.fn()}
        onStop={props.onStop}
        platformStatus={props.platformStatus}
        stagedSkillCount={props.stagedSkillCount}
        state={props.state}
        workspace={workspace(null)}
      />,
    );
    expect(screen.queryByTestId('workspace-branch')).toBeNull();
  });

  it('renders server projection facts and offers Stop only for an active Run', async () => {
    const user = userEvent.setup();
    const onStop = vi.fn();
    const active = projection();
    active.active_run = {
      completion: null,
      finished_at: null,
      run_id: 'run_000000000000000000000001',
      started_at: '2026-07-18T00:00:00Z',
      state: 'running',
    };
    active.task.title = 'Inspect projection';
    active.context_summary = {
      latest_request_id: 'req_000000000000000000000001',
      level: 'healthy',
      loaded_skill_count: 3,
      usage: {
        cache_creation_input_tokens: null,
        cached_input_ratio: null,
        cached_input_tokens: null,
        cost: null,
        elapsed_ms: 250,
        input_tokens: 50,
        output_tokens: 10,
        request_count: 1,
      },
    };
    const { rerender } = renderAt(1440, { onStop, state: snapshot(active) });

    expect(screen.getByText('Inspect projection')).toBeVisible();
    expect(screen.getByText('Fixture Server')).toBeVisible();
    expect(screen.getByText('3 loaded')).toBeVisible();
    expect(screen.getByText('2 staged')).toBeVisible();
    expect(screen.getByText('Running')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Stop run' }));
    expect(onStop).toHaveBeenCalledTimes(1);

    const idle = projection();
    idle.active_run = null;
    rerender(<WorkbenchShell {...shellProps({ state: snapshot(idle) })} />);
    expect(screen.queryByRole('button', { name: 'Stop run' })).toBeNull();
  });

  it('accepts stable slot content without changing shell ownership', () => {
    const taskNavigation: ReactNode = <p>Typed navigation slot</p>;
    renderAt(1440, { taskNavigation });
    expect(screen.getByText('Typed navigation slot')).toBeVisible();
  });
});
