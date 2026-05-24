import type { Meta, StoryObj } from "@storybook/react";
import { Button } from "./Button";

const meta: Meta<typeof Button> = {
  title: "UI/Button",
  component: Button,
};

export default meta;
type Story = StoryObj<typeof Button>;

export const Primary: Story = {
  args: { variant: "primary", size: "md", children: "Send" },
};

export const Secondary: Story = {
  args: { variant: "secondary", size: "md", children: "Cancel" },
};

export const Ghost: Story = {
  args: { variant: "ghost", size: "md", children: "More" },
};

export const Danger: Story = {
  args: { variant: "danger", size: "md", children: "Delete" },
};

export const Small: Story = {
  args: { variant: "primary", size: "sm", children: "Small" },
};

export const Disabled: Story = {
  args: { variant: "primary", size: "md", children: "Disabled", disabled: true },
};
