import type { Meta, StoryObj } from "@storybook/react";
import { Composer } from "./Composer";

const meta = {
  title: "Chat/Composer",
  component: Composer,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div className="max-w-3xl">
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof Composer>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Disabled: Story = {
  args: { disabled: true },
};

export const WithError: Story = {
  args: {
    error: "Cannot connect to model provider. Please check your API key.",
  },
};

export const WithLongError: Story = {
  args: {
    error:
      "Rate limit exceeded for claude-opus-4-7. Please wait 30 seconds before retrying or switch to a different model.",
  },
};
