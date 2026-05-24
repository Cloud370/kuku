import type { Meta, StoryObj } from "@storybook/react";
import { DiffViewer } from "./DiffViewer";

const meta = {
  title: "Right Panel/DiffViewer",
  component: DiffViewer,
  parameters: { layout: "fullscreen" },
  decorators: [
    (Story) => (
      <div className="h-screen" style={{ maxWidth: "700px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof DiffViewer>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Unified: Story = {};

export const SplitMode: Story = {
  args: {},
  render: () => {
    // starts in unified mode, user can toggle
    return <DiffViewer />;
  },
};
