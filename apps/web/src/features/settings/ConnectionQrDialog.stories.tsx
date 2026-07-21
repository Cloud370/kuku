import type { Meta, StoryObj } from '@storybook/react';

import platformStatusJson from '../../api/generated/fixtures/platform_status.json';
import type { PlatformStatus } from '../../api/generated';
import { ConnectionQrDialog } from './ConnectionQrDialog';

const info = (structuredClone(platformStatusJson) as PlatformStatus).connection;

const meta = {
  component: ConnectionQrDialog,
  title: 'Experience/Settings/ConnectionQr',
} satisfies Meta<typeof ConnectionQrDialog>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Open: Story = {
  args: {
    credentials: { current: () => 'storybook-credential' },
    encoder: {
      encode: () =>
        Promise.resolve(
          'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M/wHwAF/gL+XcwAAAAASUVORK5CYII=',
        ),
    },
    info,
    open: true,
  },
};

export const Closed: Story = {
  args: {
    credentials: { current: () => 'storybook-credential' },
    info,
    open: false,
  },
};

export const Unavailable: Story = {
  args: {
    credentials: { current: () => null },
    info,
    open: true,
  },
};
