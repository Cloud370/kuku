import type { Meta, StoryObj } from "@storybook/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ConnectionGate } from "./ConnectionGate";

const meta: Meta<typeof ConnectionGate> = {
  title: "App/ConnectionGate",
  component: ConnectionGate,
};

export default meta;
type Story = StoryObj<typeof ConnectionGate>;

export const Loading: Story = {
  render: () => {
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    return (
      <QueryClientProvider client={qc}>
        <ConnectionGate>
          <div>Content</div>
        </ConnectionGate>
      </QueryClientProvider>
    );
  },
};

export const Connected: Story = {
  render: () => {
    const qc = new QueryClient();
    qc.setQueryData(["health"], { ok: true, version: "0.1.0" });
    return (
      <QueryClientProvider client={qc}>
        <ConnectionGate>
          <div className="p-8 text-[var(--color-text-primary)]">Connected — app content here</div>
        </ConnectionGate>
      </QueryClientProvider>
    );
  },
};
