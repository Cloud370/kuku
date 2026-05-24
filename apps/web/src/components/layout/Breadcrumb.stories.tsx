import type { Meta, StoryObj } from "@storybook/react";
import { Breadcrumb } from "./Breadcrumb";

const meta = {
  title: "Layout/Breadcrumb",
  component: Breadcrumb,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Breadcrumb>;

export default meta;
type Story = StoryObj<typeof meta>;

export const SingleLevel: Story = {
  args: {
    path: [{ id: "sess-parent", label: "API Gateway Config" }],
  },
};

export const TwoLevels: Story = {
  args: {
    path: [
      { id: "sess-parent", label: "API Gateway Config" },
      { id: "sess-child", label: "Refactor Auth Module" },
    ],
  },
};

export const DeepNested: Story = {
  args: {
    path: [
      { id: "sess-root", label: "Project Setup" },
      { id: "sess-l1", label: "Database Migration" },
      { id: "sess-l2", label: "User Schema Update" },
    ],
  },
};
