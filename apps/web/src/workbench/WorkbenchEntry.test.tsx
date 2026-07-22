import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import platformStatusJson from '../api/generated/fixtures/platform_status.json';
import taskProjectionJson from '../api/generated/fixtures/task_projection.json';
import type { PlatformStatus, TaskProjection } from '../api/generated';
import { createWorkbenchSnapshot, applyProjection, selectTimelineItems } from './state';
import { WorkbenchEntry } from './WorkbenchEntry';

const snapshot = applyProjection(
  createWorkbenchSnapshot(),
  structuredClone(taskProjectionJson) as TaskProjection,
);
const store = {
  abandonConflictedCommand: vi.fn(),
  createTask: vi.fn(),
  loadOlder: vi.fn(),
  pendingCommand: null,
  reconnectFromCursor: vi.fn(),
  respond: vi.fn(),
  retryPendingCommand: vi.fn(),
  returnToRecent: vi.fn(),
  setDraft: vi.fn(),
  stopRun: vi.fn(),
  submitRun: vi.fn(),
};

vi.mock('./entry/EntryGate', () => ({
  EntryGate: ({ renderWorkbench }: { renderWorkbench: (status: PlatformStatus) => ReactNode }) =>
    renderWorkbench(structuredClone(platformStatusJson) as PlatformStatus),
}));

vi.mock('./WorkbenchController', () => ({
  WorkbenchController: ({ children }: { children: (view: unknown) => ReactNode }) =>
    children({
      catalog: {
        agents: [],
        api_version: 1,
        revision: 'catalog',
        skills: [],
        tiers: [],
        tools: [],
      },
      catalogError: null,
      loadOlder: vi.fn(),
      onRetry: vi.fn(),
      platform: structuredClone(platformStatusJson),
      selectTask: vi.fn(),
      retryCatalog: vi.fn(),
      searchCatalog: vi.fn(),
      snapshot,
      store,
      timelineItems: selectTimelineItems(snapshot),
      workspace: null,
    }),
}));

vi.mock('./components/TaskNavigation', () => ({
  TaskNavigation: () => <p>Task list</p>,
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('WorkbenchEntry', () => {
  beforeEach(() => {
    window.ResizeObserver = class implements ResizeObserver {
      disconnect = vi.fn();
      observe = vi.fn();
      unobserve = vi.fn();
    };
    window.matchMedia = vi.fn().mockImplementation(
      (query: string) =>
        ({
          addEventListener: vi.fn(),
          addListener: vi.fn(),
          dispatchEvent: vi.fn(),
          matches: false,
          media: query,
          onchange: null,
          removeEventListener: vi.fn(),
          removeListener: vi.fn(),
        }) satisfies MediaQueryList,
    );
  });

  it('mounts Tasks, Chat, Context, and the production Composer', () => {
    render(
      <WorkbenchEntry
        context={<p>Context surface</p>}
        onOpenAgentThread={vi.fn()}
        onOpenContext={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenLoadedSkills={vi.fn()}
        onOpenRequestContext={vi.fn()}
        onOpenReview={vi.fn()}
      />,
    );

    expect(screen.getByRole('navigation', { name: 'Tasks' })).toBeVisible();
    expect(screen.getByRole('main', { name: 'Chat' })).toBeVisible();
    expect(screen.getByRole('complementary', { name: 'Agent Context' })).toBeVisible();
    expect(screen.getByRole('textbox', { name: 'Message' })).toBeVisible();
    expect(screen.getByRole('status', { name: 'Run status' })).toBeInTheDocument();
    const composer = screen.getByLabelText('Composer');
    expect(composer.parentElement).toHaveClass('flex', 'h-full', 'min-h-0', 'flex-col');
    expect(composer.previousElementSibling).toHaveClass('min-h-0', 'flex-1', 'overflow-y-auto');
    expect(composer.previousElementSibling).toHaveAttribute('data-chat-scroll');
  });

  it('renders the Context slot from the current Workbench view when provided', () => {
    render(
      <WorkbenchEntry
        context={<p>Fallback Context</p>}
        onOpenAgentThread={vi.fn()}
        onOpenContext={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenLoadedSkills={vi.fn()}
        onOpenRequestContext={vi.fn()}
        onOpenReview={vi.fn()}
        renderContext={(view) => <p>Context for {view.snapshot.selectedTaskId}</p>}
      />,
    );

    expect(screen.getByText('Context for tsk_000000000000000000000001')).toBeVisible();
    expect(screen.queryByText('Fallback Context')).toBeNull();
  });
});
