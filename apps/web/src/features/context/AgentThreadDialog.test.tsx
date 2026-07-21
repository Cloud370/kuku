import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { webApi } from '../../api/client';
import { AgentThreadDialog } from './AgentThreadDialog';
import { agentThreadFixture, conversationId, taskId } from './testFixtures';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('AgentThreadDialog', () => {
  it('loads a delegated thread without an input surface', async () => {
    vi.spyOn(webApi.context, 'agentThread').mockResolvedValue(agentThreadFixture());
    render(
      <AgentThreadDialog
        conversationId={conversationId}
        onClose={vi.fn()}
        onSelectRequest={vi.fn()}
        open
        taskId={taskId}
      />,
    );

    expect(await screen.findByText('Research result')).toBeVisible();
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.queryByRole('button', { name: /send|submit|stop/i })).toBeNull();
    expect(screen.getAllByRole('button', { name: /request req_/i })).toHaveLength(2);
  });

  it('renders the bounded-history notice and closes on Escape', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    vi.spyOn(webApi.context, 'agentThread').mockResolvedValue(
      agentThreadFixture({ messages_truncated_before: true }),
    );
    render(
      <AgentThreadDialog
        conversationId={conversationId}
        onClose={onClose}
        onSelectRequest={vi.fn()}
        open
        taskId={taskId}
      />,
    );

    expect(await screen.findByText('Earlier delegated messages are not shown')).toBeVisible();
    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('renders canonical missing-thread errors without inventing controls', async () => {
    vi.spyOn(webApi.context, 'agentThread').mockRejectedValue(
      Object.assign(new Error('Conversation not found'), { code: 'conversation_not_found' }),
    );
    render(
      <AgentThreadDialog
        conversationId={conversationId}
        onClose={vi.fn()}
        onSelectRequest={vi.fn()}
        open
        taskId={taskId}
      />,
    );

    expect(await screen.findByText('conversation_not_found')).toBeVisible();
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.queryByRole('button', { name: /load more/i })).toBeNull();
  });
});
