import type { Meta, StoryObj } from "@storybook/react";
import { SessionList } from "./SessionList";

const meta = {
  title: "Layout/SessionList",
  component: SessionList,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div style={{ maxWidth: "300px", height: "500px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof SessionList>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const ActiveSession: Story = {
  args: {
    activeId: "sess-a3f2",
  },
};

export const Empty: Story = {
  args: {
    groups: [],
  },
};

export const SingleGroupToday: Story = {
  args: {
    groups: [
      {
        label: "Today",
        sessions: [
          { id: "sess-x1y2", preview: "Write unit tests for the user service module", status: "running" },
          { id: "sess-z3w4", preview: "Review PR feedback on database migration", status: "completed" },
        ],
      },
    ],
  },
};
