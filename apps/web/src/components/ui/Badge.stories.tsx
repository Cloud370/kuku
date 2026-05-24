import type { Meta, StoryObj } from "@storybook/react";
import { Badge } from "./Badge";

const meta: Meta<typeof Badge> = {
  title: "UI/Badge",
  component: Badge,
};

export default meta;
type Story = StoryObj<typeof Badge>;

export const Default: Story = {
  args: { children: "default" },
};

export const Warning: Story = {
  args: { variant: "warning", children: "command" },
};

export const Error: Story = {
  args: { variant: "error", children: "failed" },
};

export const Success: Story = {
  args: { variant: "success", children: "done" },
};
