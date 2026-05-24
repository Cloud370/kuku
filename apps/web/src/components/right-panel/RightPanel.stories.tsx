import type { Meta, StoryObj } from "@storybook/react";
import { RightPanel } from "./RightPanel";
import { useUIStore } from "@/stores/ui";

const meta = {
  title: "Right Panel/RightPanel",
  component: RightPanel,
  parameters: { layout: "fullscreen" },
  decorators: [
    (Story) => {
      useUIStore.setState({ rightPanelTab: "diff" });
      return (
        <div className="h-screen" style={{ maxWidth: "400px" }}>
          <Story />
        </div>
      );
    },
  ],
} satisfies Meta<typeof RightPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const TerminalActive: Story = {
  decorators: [
    (Story) => {
      useUIStore.setState({ rightPanelTab: "terminal" });
      return (
        <div className="h-screen" style={{ maxWidth: "400px" }}>
          <Story />
        </div>
      );
    },
  ],
};

export const StatusActive: Story = {
  decorators: [
    (Story) => {
      useUIStore.setState({ rightPanelTab: "status" });
      return (
        <div className="h-screen" style={{ maxWidth: "400px" }}>
          <Story />
        </div>
      );
    },
  ],
};
