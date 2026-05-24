import type { Meta, StoryObj } from "@storybook/react";
import { WorkspaceZone } from "./WorkspaceZone";
import { useUIStore } from "@/stores/ui";

const meta = {
  title: "Layout/WorkspaceZone",
  component: WorkspaceZone,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => {
      useUIStore.setState({ workspace: "/home/cloud/projects/kuku" });
      return (
        <div style={{ maxWidth: "260px", height: "200px" }}>
          <Story />
        </div>
      );
    },
  ],
} satisfies Meta<typeof WorkspaceZone>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const WorkSelected: Story = {
  decorators: [
    (Story) => {
      useUIStore.setState({ workspace: "/home/cloud/projects/work" });
      return (
        <div style={{ maxWidth: "260px", height: "200px" }}>
          <Story />
        </div>
      );
    },
  ],
};

export const OssSelected: Story = {
  decorators: [
    (Story) => {
      useUIStore.setState({ workspace: "/home/cloud/projects/oss" });
      return (
        <div style={{ maxWidth: "260px", height: "200px" }}>
          <Story />
        </div>
      );
    },
  ],
};
