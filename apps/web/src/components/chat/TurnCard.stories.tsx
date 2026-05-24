import type { Meta, StoryObj } from "@storybook/react";
import { TurnCard } from "./TurnCard";

const meta = {
  title: "Chat/TurnCard",
  component: TurnCard,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div className="max-w-2xl space-y-4">
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof TurnCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const UserMessage: Story = {
  args: {
    role: "user",
    children: "How do I configure the API gateway to handle rate limiting?",
  },
};

export const AgentMessage: Story = {
  args: {
    role: "agent",
    children: "Let me check your current Dockerfile and project structure first.",
  },
};

export const AgentWithCode: Story = {
  args: {
    role: "agent",
    children: (
      <div>
        <p className="mb-2">I found several issues in your Dockerfile:</p>
        <ol className="list-decimal list-inside space-y-1">
          <li>Missing multi-stage build separation</li>
          <li>Build dependencies not cleaned up</li>
          <li>
            No{" "}
            <code className="font-mono text-[var(--text-xs)] bg-[var(--color-surface)] px-1 rounded">
              .dockerignore
            </code>{" "}
            file
          </li>
        </ol>
      </div>
    ),
  },
};

export const ConversationThread: Story = {
  args: { role: "user" as const },
  render: () => (
    <div className="flex flex-col gap-4">
      <TurnCard role="user">
        <p>How do I configure the API gateway?</p>
      </TurnCard>
      <TurnCard role="agent">
        <p>I&apos;ll help you configure the API gateway. Let me first check your current setup.</p>
        <div className="mt-3 p-3 rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)]">
          <p className="text-[var(--text-xs)] text-[var(--color-text-muted)] font-mono">
            $ cat /etc/api-gateway/config.yaml
          </p>
        </div>
      </TurnCard>
      <TurnCard role="user">
        <p>Actually, I&apos;m using the new v2 config format.</p>
      </TurnCard>
      <TurnCard role="agent">
        <p>Got it. The v2 format uses TOML instead of YAML. Let me check your v2 config.</p>
      </TurnCard>
    </div>
  ),
};

export const AgentWithThinking: Story = {
  args: { role: "agent" as const },
  render: () => (
    <TurnCard role="agent">
      <details className="mb-3">
        <summary className="text-[var(--text-xs)] text-[var(--color-text-muted)] cursor-pointer select-none">
          Reasoning
        </summary>
        <p className="mt-2 text-[var(--text-xs)] text-[var(--color-text-muted)] bg-[var(--color-surface)] p-2 rounded-[var(--radius-sm)] font-mono">
          The user wants to optimize their Docker build. Key areas to check: multi-stage
          builds, layer caching, binary size optimization with strip and LTO.
        </p>
      </details>
      <p>Here are the optimizations I recommend for your Docker build...</p>
    </TurnCard>
  ),
};
