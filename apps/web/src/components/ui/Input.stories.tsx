import type { Meta, StoryObj } from "@storybook/react";
import { Input } from "./Input";

const meta: Meta<typeof Input> = {
  title: "UI/Input",
  component: Input,
};

export default meta;
type Story = StoryObj<typeof Input>;

export const Default: Story = {
  args: { placeholder: "Ask anything..." },
};

export const WithValue: Story = {
  args: { value: "Hello world", readOnly: true },
};

export const Ghost: Story = {
  args: { variant: "ghost", placeholder: "Ghost input..." },
};

export const Disabled: Story = {
  args: { placeholder: "Disabled", disabled: true },
};
