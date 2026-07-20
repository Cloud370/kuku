import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import platformStatusJson from '../api/generated/fixtures/platform_status.json';
import type { PlatformStatus } from '../api/generated';
import { WorkbenchEntry } from './WorkbenchEntry';
import { createWorkbenchSnapshot } from './state';

vi.mock('./entry/EntryGate', () => ({
  EntryGate: ({ renderWorkbench }: { renderWorkbench: (status: PlatformStatus) => ReactNode }) =>
    renderWorkbench(structuredClone(platformStatusJson) as PlatformStatus),
}));

afterEach(() => {
  cleanup();
});

describe('WorkbenchEntry', () => {
  beforeEach(() => {
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

  it('forwards thread and review callbacks into the chat slot', async () => {
    const user = userEvent.setup();
    const onOpenAgentThread = vi.fn();
    const onOpenReview = vi.fn();
    const chat = vi.fn(
      ({
        onOpenAgentThread: openThread,
        onOpenReview: openReview,
      }: {
        onOpenAgentThread: (taskId: string, conversationId: string) => void;
        onOpenReview: (taskId: string) => void;
      }) => (
        <>
          <button
            onClick={() => {
              openThread('task-1', 'conversation-1');
            }}
            type="button"
          >
            Open thread
          </button>
          <button
            onClick={() => {
              openReview('task-1');
            }}
            type="button"
          >
            Open review
          </button>
        </>
      ),
    );

    render(
      <WorkbenchEntry
        chat={chat}
        context={<div>Context</div>}
        onOpenAgentThread={onOpenAgentThread}
        onOpenContext={vi.fn()}
        onOpenReview={onOpenReview}
        onStop={vi.fn()}
        stagedSkillCount={0}
        state={createWorkbenchSnapshot()}
        taskNavigation={<div>Tasks</div>}
        workspace={null}
      />,
    );

    expect(chat).toHaveBeenCalledWith(expect.objectContaining({ onOpenAgentThread, onOpenReview }));
    await user.click(screen.getByRole('button', { name: 'Open thread' }));
    await user.click(screen.getByRole('button', { name: 'Open review' }));
    expect(onOpenAgentThread).toHaveBeenCalledWith('task-1', 'conversation-1');
    expect(onOpenReview).toHaveBeenCalledWith('task-1');
  });
});
