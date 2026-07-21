import type { Meta, StoryObj } from '@storybook/react';

import { ExactRequestDialog } from './ExactRequestDialog';
import { contextFixture } from './testFixtures';

const meta: Meta<typeof ExactRequestDialog> = {
  title: 'Experience/Context',
  component: ExactRequestDialog,
};

export default meta;
type Story = StoryObj<typeof ExactRequestDialog>;

const fixture = contextFixture();
if (fixture.exact_request === null) throw new Error('exact Request fixture is missing');
const exactRequest = fixture.exact_request;

export const Historical: Story = {
  args: {
    exactPayloadHash: fixture.exact_payload_hash,
    exactRequest,
    onClose: () => undefined,
    open: true,
  },
};
