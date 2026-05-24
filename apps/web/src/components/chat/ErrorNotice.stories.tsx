import type { Meta, StoryObj } from "@storybook/react";
import { ErrorNotice } from "./ErrorNotice";
import { TurnCard } from "./TurnCard";

const meta = {
  title: "Chat/ErrorNotice",
  component: ErrorNotice,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div className="max-w-2xl">
        <TurnCard role="agent">
          <Story />
        </TurnCard>
      </div>
    ),
  ],
} satisfies Meta<typeof ErrorNotice>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Network: Story = {
  args: { type: "provider_network", message: "ETIMEDOUT: connect to api.example.com:443" },
};

export const Auth: Story = {
  args: { type: "provider_auth", message: "HTTP 401: Invalid API key" },
};

export const RateLimit: Story = {
  args: { type: "provider_rate_limit", message: "Retry after 30 seconds" },
};

export const Overflow: Story = {
  args: { type: "provider_overflow", message: "Context length 128000 exceeds model limit of 100000 tokens" },
};

export const Internal: Story = {
  args: { type: "internal" },
};
