import type { Meta, StoryObj } from "@storybook/react";
import { FileAccessBar } from "./FileAccessBar";
import { TurnCard } from "./TurnCard";

const meta = {
  title: "Chat/FileAccessBar",
  component: FileAccessBar,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div className="max-w-2xl">
        <TurnCard role="agent">
          <Story />
          <p className="text-[var(--text-sm)] text-[var(--color-text-secondary)]">
            Based on these files, here are my recommendations...
          </p>
        </TurnCard>
      </div>
    ),
  ],
} satisfies Meta<typeof FileAccessBar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const SingleFile: Story = {
  args: {
    files: [
      {
        name: "src/main.rs",
        lines: 89,
        snippet: "fn main() {\n    let config = load_config()?;\n    let server = Server::new(config);\n    server.run().await?;\n}",
      },
    ],
  },
};

export const EmptySnippet: Story = {
  args: {
    files: [
      {
        name: "empty.txt",
        lines: 0,
        snippet: "",
      },
    ],
  },
};
