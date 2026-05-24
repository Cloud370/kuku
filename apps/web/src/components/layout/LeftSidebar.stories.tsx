import type { Meta, StoryObj } from "@storybook/react";
import { LeftSidebar } from "./LeftSidebar";

const meta = {
  title: "Layout/LeftSidebar",
  component: LeftSidebar,
  parameters: {
    layout: "padded",
  },
  decorators: [
    (Story) => (
      <div style={{ height: "600px", maxWidth: "280px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof LeftSidebar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
