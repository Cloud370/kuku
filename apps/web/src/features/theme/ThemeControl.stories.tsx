import type { Meta, StoryObj } from '@storybook/react';

import { ThemeControl } from './ThemeControl';

const meta = {
  component: ThemeControl,
  title: 'Experience/ThemeControl',
} satisfies Meta<typeof ThemeControl>;

export default meta;
type Story = StoryObj<typeof meta>;

export const SegmentedControl: Story = {};
