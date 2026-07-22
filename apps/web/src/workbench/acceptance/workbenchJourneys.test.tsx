import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import platformStatusJson from '../../api/generated/fixtures/platform_status.json';
import type { PlatformStatus } from '../../api/generated';
import { WorkbenchEntry } from '../WorkbenchEntry';
import { createFakeWorkbenchServer } from '../test/fakeServer';

vi.mock('../entry/EntryGate', () => ({
  EntryGate: ({ renderWorkbench }: { renderWorkbench: (status: PlatformStatus) => ReactNode }) =>
    renderWorkbench(structuredClone(platformStatusJson) as PlatformStatus),
}));

afterEach(() => {
  cleanup();
});

describe('Workbench journeys', () => {
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

  it('loads a Task and submits the message with canonical Tier and Skill IDs', async () => {
    const user = userEvent.setup();
    const server = createFakeWorkbenchServer();
    render(
      <WorkbenchEntry
        api={server.api}
        context={<p>Context</p>}
        onOpenAgentThread={vi.fn()}
        onOpenContext={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenLoadedSkills={vi.fn()}
        onOpenRequestContext={vi.fn()}
        onOpenReview={vi.fn()}
        onOpenSettings={vi.fn()}
        route={{ kind: 'task', taskId: server.taskId }}
      />,
    );

    const message = await screen.findByRole('textbox', { name: 'Message' });
    await waitFor(() => {
      expect(message).toBeEnabled();
    });
    expect(message).toBeInTheDocument();
    message.focus();
    await user.type(message, 'inspect workspace', { skipClick: true });
    expect(message).toHaveValue('inspect workspace');
    await user.click(screen.getByRole('button', { name: 'Add Skill' }));
    expect(message).toHaveValue('inspect workspace');
    await user.click(screen.getByRole('option', { name: 'rust-review' }));
    expect(message).toHaveValue('inspect workspace');
    const send = screen.getByRole('button', { name: 'Send' });
    expect(send).toBeEnabled();
    await user.click(send);

    await waitFor(() => {
      expect(server.submitBodies).toHaveLength(1);
    });
    expect(server.submitBodies[0]).toMatchObject({
      message: 'inspect workspace',
      skill_ids: ['skill:project:rust-review'],
      tier_id: 'tier:balanced',
    });
  });
});
