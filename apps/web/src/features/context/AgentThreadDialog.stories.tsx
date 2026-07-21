import type { Meta, StoryObj } from '@storybook/react';

import { AgentThreadDialog } from './AgentThreadDialog';
import { agentThreadFixture, conversationId, taskId } from './testFixtures';

const meta: Meta<typeof AgentThreadDialog> = {
  title: 'Experience/Context',
  component: AgentThreadDialog,
};

export default meta;
type Story = StoryObj<typeof AgentThreadDialog>;

export const AgentThread: Story = {
  args: {
    conversationId,
    onClose: () => undefined,
    onSelectRequest: () => undefined,
    open: true,
    taskId,
    thread: agentThreadFixture({ messages_truncated_before: true }),
  },
};
