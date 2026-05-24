import type { Meta, StoryObj } from "@storybook/react";
import { AgentToolBody } from "./AgentToolBody";
import { ToolCard } from "./ToolCard";
import { TurnCard } from "./TurnCard";

const meta = {
  title: "Chat/AgentToolBody",
  component: AgentToolBody,
  parameters: { layout: "padded" },
  decorators: [
    (Story) => (
      <div className="max-w-2xl">
        <TurnCard role="agent">
          <ToolCard
            icon="🤖"
            name="task"
            summary="Sub-agent: refactor auth module"
            status="completed"
            kind="agent"
          >
            <Story />
          </ToolCard>
        </TurnCard>
      </div>
    ),
  ],
} satisfies Meta<typeof AgentToolBody>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithChildTools: Story = {
  args: {
    children: (
      <div className="space-y-2">
        <ToolCard icon="👁️" name="read_file" summary="src/auth/handler.ts (89 lines)" status="completed" />
        <ToolCard icon="✏️" name="write_file" summary="src/auth/refactored.ts (120 lines)" status="completed" />
        <ToolCard icon="💻" name="execute_command" summary="npm test -- --pass" status="completed" />
        <ToolCard icon="🔧" name="execute_command" summary="npm run lint -- --fix" status="completed" />
        <p className="text-[var(--text-xs)] text-[var(--color-text-muted)] font-mono mt-1">
          All tests passing. Lint clean. Refactor complete.
        </p>
      </div>
    ),
  },
};

export const WithNestedAgent: Story = {
  args: {
    children: (
      <div className="space-y-2">
        <ToolCard icon="👁️" name="read_file" summary="src/db/schema.ts (200 lines)" status="completed" />
        <ToolCard
          icon="🤖"
          name="task"
          summary="Sub-agent: migrate users table"
          status="completed"
          kind="agent"
        >
          <AgentToolBody>
            <div className="space-y-2">
              <ToolCard icon="✏️" name="write_file" summary="migrations/002_users.sql" status="completed" />
              <ToolCard icon="💻" name="execute_command" summary="kuku migrate up" status="completed" />
            </div>
          </AgentToolBody>
        </ToolCard>
      </div>
    ),
  },
};
