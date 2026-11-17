import type { Meta, StoryObj } from "@storybook/react";
import { LeftSidebar } from "./LeftSidebar";
import { SessionList } from "./SessionList";
import { WorkspaceZone } from "./WorkspaceZone";

const meta = {
  title: "Layout/LeftSidebar",
  component: LeftSidebar,
  parameters: { layout: "fullscreen" },
  decorators: [
    (Story) => (
      <div className="fixed inset-0" style={{ maxWidth: "280px" }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof LeftSidebar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    workspacePanel: (
      <WorkspaceZone workspace="/work/kuku" workspaces={["/work/kuku", "/work/ops"]} />
    ),
    sessions: <SessionList />,
  },
};
