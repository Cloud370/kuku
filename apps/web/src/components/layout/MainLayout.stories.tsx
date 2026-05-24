import type { Meta, StoryObj } from "@storybook/react";
import { MainLayout } from "./MainLayout";
import { useUIStore } from "@/stores/ui";

const ResetStore = () => {
  useUIStore.setState({ layoutMode: "three-column" });
  return null;
};

const meta = {
  title: "Layout/MainLayout",
  component: MainLayout,
  parameters: { layout: "fullscreen" },
  decorators: [
    (Story) => (
      <div style={{ height: "100vh" }}>
        <ResetStore />
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof MainLayout>;

export default meta;
type Story = StoryObj<typeof meta>;

export const ThreeColumn: Story = {
  render: () => (
    <MainLayout sessionTitle="kuku" sessionStatus="idle">
      <div className="flex items-center justify-center h-full text-[var(--color-text-muted)] text-[var(--text-lg)]">
        Chat Area
      </div>
    </MainLayout>
  ),
};

export const RunningSession: Story = {
  render: () => (
    <MainLayout sessionTitle="My Project" sessionStatus="running">
      <div className="flex flex-col gap-4 p-4 overflow-auto h-full">
        <div className="p-4 rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface-raised)]">
          <p className="text-[var(--text-sm)] text-[var(--color-text-secondary)]">
            User: How do I configure the API?
          </p>
        </div>
        <div className="p-4 rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface-raised)]">
          <p className="text-[var(--text-sm)] text-[var(--color-text-secondary)]">
            Agent: Let me help you configure the API...
          </p>
        </div>
      </div>
    </MainLayout>
  ),
};
