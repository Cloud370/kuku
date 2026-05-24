import type { Meta, StoryObj } from "@storybook/react";
import { PermissionDock } from "./PermissionDock";

const meta = {
  title: "Chat/PermissionDock",
  component: PermissionDock,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div className="max-w-2xl">
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof PermissionDock>;

export default meta;
type Story = StoryObj<typeof meta>;

export const FileWrite: Story = {
  args: {
    toolIcon: "✏️",
    toolName: "write_file",
    riskLabel: "mutation",
    summary: 'Write to "src/config/secrets.ts" (12 lines)',
  },
};

export const ShellCommand: Story = {
  args: {
    toolIcon: "💻",
    toolName: "execute_command",
    riskLabel: "shell",
    summary: "rm -rf ./node_modules && npm install",
  },
};

export const NoRiskLabel: Story = {
  args: {
    toolIcon: "👁️",
    toolName: "read_file",
    summary: 'Read "src/lib/utils.ts" (45 lines)',
  },
};
