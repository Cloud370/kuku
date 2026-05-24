import type { Meta, StoryObj } from "@storybook/react";
import { MainLayout } from "./MainLayout";
import { useUIStore } from "@/stores/ui";

const LayoutWrapper = ({
  children,
  ...props
}: React.ComponentProps<typeof MainLayout>) => {
  return <MainLayout {...props}>{children}</MainLayout>;
};

const meta = {
  title: "Layout/MainLayout",
  component: MainLayout,
  parameters: {
    layout: "fullscreen",
  },
  decorators: [
    (Story) => {
      useUIStore.setState({
        layoutMode: "three-column",
        leftSidebarWidth: 260,
        rightPanelWidth: 400,
      });
      return (
        <div style={{ height: "100vh" }}>
          <Story />
        </div>
      );
    },
  ],
} satisfies Meta<typeof MainLayout>;

export default meta;
type Story = StoryObj<typeof meta>;

export const ThreeColumn: Story = {
  render: () => (
    <LayoutWrapper
      sessionTitle="kuku"
      sessionStatus="idle"
    >
      <div className="flex items-center justify-center h-full text-[var(--color-text-muted)]">
        Chat Area
      </div>
    </LayoutWrapper>
  ),
};

export const WithRunningSession: Story = {
  render: () => (
    <LayoutWrapper
      sessionTitle="My Project"
      sessionStatus="running"
    >
      <div className="flex flex-col gap-4 p-4 overflow-auto h-full">
        <div className="p-4 rounded-[var(--radius-md)] border border-[var(--color-border)]">
          <p className="text-[var(--text-sm)] text-[var(--color-text-secondary)]">
            User: How do I configure the API?
          </p>
        </div>
        <div className="p-4 rounded-[var(--radius-md)] border border-[var(--color-border)]">
          <p className="text-[var(--text-sm)] text-[var(--color-text-secondary)]">
            Agent: Let me help you configure the API...
          </p>
        </div>
      </div>
    </LayoutWrapper>
  ),
};
