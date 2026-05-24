import type { Meta, StoryObj } from "@storybook/react";
import { ThinkingBlock } from "./ThinkingBlock";
import { TurnCard } from "./TurnCard";

const meta = {
  title: "Chat/ThinkingBlock",
  component: ThinkingBlock,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div className="max-w-2xl">
        <TurnCard role="agent">
          <Story />
          <p className="text-[var(--text-sm)] text-[var(--color-text-secondary)]">
            The optimal configuration for your use case is multi-stage builds with layer caching.
          </p>
        </TurnCard>
      </div>
    ),
  ],
} satisfies Meta<typeof ThinkingBlock>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Collapsed: Story = {
  args: {
    children:
      "The user wants Docker build optimization. Key areas: multi-stage builds reduce final image size, layer caching speeds up rebuilds, .dockerignore prevents unnecessary context upload. The current Dockerfile is a single stage with all build dependencies included in the final image.",
  },
};

export const OpenByDefault: Story = {
  args: {
    defaultOpen: true,
    children:
      "Analyzing the rate limiting strategy: token bucket vs sliding window. Token bucket allows bursts but may cause uneven load. Sliding window provides smoother distribution but requires more state. For API gateway use case, sliding window with Redis backend is recommended for accurate per-client quotas.",
  },
};
