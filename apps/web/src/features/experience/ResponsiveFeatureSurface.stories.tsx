import type { Meta, StoryObj } from '@storybook/react';

import { ResponsiveFeatureSurface } from './ResponsiveFeatureSurface';

const meta = {
  component: ResponsiveFeatureSurface,
  render: (args) => (
    <ResponsiveFeatureSurface {...args}>
      <p>
        src/features/experience/a-very-long-unbroken-path-segment-that-must-wrap-without-growing-the-page.tsx
      </p>
      <pre className="mt-4 overflow-auto">
        {'const result = await inspectWorkspace();\n'.repeat(40)}
      </pre>
    </ResponsiveFeatureSurface>
  ),
  title: 'Experience/ResponsiveFeatureSurface',
} satisfies Meta<typeof ResponsiveFeatureSurface>;

export default meta;
type Story = StoryObj<typeof meta>;

const baseArgs = {
  children: null,
  kind: 'review' as const,
  mode: 'full' as const,
  toolbar: <button type="button">Close</button>,
};

export const Phone360: Story = {
  args: baseArgs,
  decorators: [
    (Story) => (
      <div style={{ height: 720, width: 360 }}>
        <Story />
      </div>
    ),
  ],
};
export const Tablet768: Story = {
  args: baseArgs,
  decorators: [
    (Story) => (
      <div style={{ height: 900, width: 768 }}>
        <Story />
      </div>
    ),
  ],
};
export const Desktop1440: Story = {
  args: baseArgs,
  decorators: [
    (Story) => (
      <div style={{ height: 900, width: 1440 }}>
        <Story />
      </div>
    ),
  ],
};
export const Zoom200: Story = { args: baseArgs };
export const ForcedColors: Story = { args: baseArgs };
export const WebKit360: Story = { ...Phone360 };
export const SafeAreaInsets: Story = { args: baseArgs };
