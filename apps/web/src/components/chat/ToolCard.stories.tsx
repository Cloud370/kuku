import type { Meta, StoryObj } from "@storybook/react";
import { ToolCard } from "./ToolCard";
import { TurnCard } from "./TurnCard";

const meta = {
  title: "Chat/ToolCard",
  component: ToolCard,
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
} satisfies Meta<typeof ToolCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Running: Story = {
  args: {
    icon: "👁️",
    name: "read_file",
    summary: "Reading src/api/gateway.ts...",
    status: "running",
  },
};

export const Completed: Story = {
  args: {
    icon: "👁️",
    name: "read_file",
    summary: "src/api/gateway.ts (142 lines)",
    status: "completed",
    children: (
      <pre className="font-mono text-[var(--text-xs)] text-[var(--color-text-secondary)] p-2 bg-[var(--color-surface)] rounded-[var(--radius-sm)] border border-[var(--color-border)] overflow-x-auto">
{`export class Gateway {
  private rateLimiter: RateLimiter;
  constructor(config: GatewayConfig) {
    this.rateLimiter = new RateLimiter(config.limit);
  }
}`}
      </pre>
    ),
  },
};

export const Error: Story = {
  args: {
    icon: "🔧",
    name: "execute_command",
    summary: "exit code 1: permission denied",
    status: "error",
  },
};

export const AgentTool: Story = {
  args: {
    icon: "🤖",
    name: "task",
    summary: "Sub-agent: refactor auth module",
    status: "completed",
    kind: "agent",
    childSessionId: "sess-child-001",
  },
};

export const WithDetailText: Story = {
  args: {
    icon: "💻",
    name: "execute_command",
    summary: "cargo build --release (exit 0)",
    status: "completed",
    children: (
      <pre className="font-mono text-[var(--text-xs)] text-[var(--color-text-secondary)] p-2 bg-[var(--color-surface)] rounded-[var(--radius-sm)] border border-[var(--color-border)]">
{`Compiling kuku v0.1.0
   Compiling kuku-cli v0.1.0
   Compiling kuku-server v0.1.0
    Finished release [optimized] in 12.34s`}
      </pre>
    ),
  },
};
