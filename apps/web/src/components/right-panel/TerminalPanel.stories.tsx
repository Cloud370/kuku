import type { Meta, StoryObj } from "@storybook/react";
import { TerminalPanel } from "./TerminalPanel";

const meta = {
  title: "Right Panel/TerminalPanel",
  component: TerminalPanel,
  parameters: { layout: "fullscreen" },
  decorators: [
    (Story) => (
      <div className="h-screen" style={{ maxWidth: "500px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof TerminalPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Empty: Story = {
  args: { lines: [] },
};
