import type { Meta, StoryObj } from "@storybook/react";
import { WorkspaceZone } from "./WorkspaceZone";

const workspaces = [
  "/home/cloud/projects/kuku",
  "/home/cloud/projects/work",
  "/home/cloud/projects/oss",
];

const meta = {
  title: "Layout/WorkspaceZone",
  component: WorkspaceZone,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div style={{ maxWidth: "260px", height: "200px" }}>
        <Story />
      </div>
    ),
  ],
  args: { workspaces, workspace: workspaces[0] },
} satisfies Meta<typeof WorkspaceZone>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const WorkSelected: Story = { args: { workspace: workspaces[1] } };

export const WorkspaceError: Story = {
  args: { error: "The selected server directory could not be opened." },
};
