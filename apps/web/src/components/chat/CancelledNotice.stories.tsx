import type { Meta, StoryObj } from "@storybook/react";
import { CancelledNotice } from "./CancelledNotice";
import { TurnCard } from "./TurnCard";

const meta = {
  title: "Chat/CancelledNotice",
  component: CancelledNotice,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div className="max-w-2xl">
        <TurnCard role="agent">
          <p className="text-[var(--text-sm)] text-[var(--color-text-secondary)] mb-2">
            Let me check the API configuration...
          </p>
          <Story />
        </TurnCard>
      </div>
    ),
  ],
} satisfies Meta<typeof CancelledNotice>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const WithTurnNumber: Story = {
  args: { turnNumber: 3 },
};
