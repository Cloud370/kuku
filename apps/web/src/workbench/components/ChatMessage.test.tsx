import '@testing-library/jest-dom/vitest';

import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { MessageProjection } from '../../api/generated';
import { ChatMessage } from './ChatMessage';

afterEach(cleanup);

describe('ChatMessage', () => {
  it.each(['user', 'agent'] as const)('uses readable body typography for %s messages', (role) => {
    const message = {
      file_references: [],
      finalized: true,
      message_id: `msg_${role}`,
      order_key: 1,
      request_ids: [],
      role,
      text: 'A clear conversation message',
    } satisfies MessageProjection;

    render(
      <ChatMessage
        message={message}
        onOpenFile={vi.fn()}
        onOpenRequestContext={vi.fn()}
        taskId="tsk_000000000000000000000001"
      />,
    );

    expect(screen.getByRole('article').firstElementChild).toHaveClass(
      'text-[15px]',
      'leading-7',
      'text-[var(--color-text-primary)]',
    );
  });
});
