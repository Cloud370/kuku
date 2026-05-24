import type { Meta, StoryObj } from "@storybook/react";
import { StatusPanel } from "./StatusPanel";

const meta = {
  title: "Right Panel/StatusPanel",
  component: StatusPanel,
  parameters: { layout: "fullscreen" },
  decorators: [
    (Story) => (
      <div className="h-screen" style={{ maxWidth: "450px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof StatusPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const NoEvents: Story = {
  args: { events: [] },
};
