import type { Meta, StoryObj } from "@storybook/react";
import { Header } from "./Header";

const meta = {
  title: "Layout/Header",
  component: Header,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Header>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const RunningSession: Story = {
  args: {
    sessionTitle: "My Project",
    sessionStatus: "running",
  },
};

export const WaitingSession: Story = {
  args: {
    sessionTitle: "API Migration",
    sessionStatus: "waiting",
  },
};

export const CompletedSession: Story = {
  args: {
    sessionTitle: "Refactor Auth",
    sessionStatus: "completed",
  },
};

export const OneColumnMode: Story = {
  args: {
    layoutMode: "one-column",
  },
};

export const TwoColumnMode: Story = {
  args: {
    layoutMode: "two-column",
  },
};
