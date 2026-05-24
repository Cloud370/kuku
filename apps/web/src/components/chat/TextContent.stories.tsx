import type { Meta, StoryObj } from "@storybook/react";
import { TextContent } from "./TextContent";
import { TurnCard } from "./TurnCard";

const meta = {
  title: "Chat/TextContent",
  component: TextContent,
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
} satisfies Meta<typeof TextContent>;

export default meta;
type Story = StoryObj<typeof meta>;

export const PlainText: Story = {
  args: {
    text: "Here is the configuration you need. Add this to your `gateway.toml` file and restart the server.",
  },
};

export const MarkdownWithCode: Story = {
  args: {
    text: [
      "## API Gateway Configuration",
      "",
      "To set up rate limiting, add the following to your config:",
      "",
      "```toml",
      "[gateway]",
      'port = 8080',
      'rate_limit = 1000',
      'backend_url = "http://localhost:3001"',
      "```",
      "",
      "Then restart:",
      "",
      "```bash",
      "$ systemctl restart api-gateway",
      "```",
      "",
      "> **Note:** Rate limits are applied per-client. Adjust based on your traffic patterns.",
    ].join("\n"),
  },
};

export const MarkdownWithList: Story = {
  args: {
    text: [
      "Here are the recommended optimizations:",
      "",
      "1. **Multi-stage builds** — reduces final image size",
      "2. **Layer caching** — speeds up rebuilds",
      "3. **`.dockerignore`** — prevents unnecessary context upload",
      "4. **`cargo chef`** — caches dependencies separately",
      "",
      "Each of these can reduce build time by 30-50%.",
    ].join("\n"),
  },
};

export const InlineCode: Story = {
  args: {
    text: [
      "The `Gateway` class accepts a `GatewayConfig` object. Use `rateLimiter.enable()` to activate rate limiting.",
      "",
      "Key types:",
      "- `RateLimiter` — core limiting logic",
      "- `GatewayConfig` — server and limit settings",
      "- `RateLimitExceeded` — error thrown on limit breach",
    ].join("\n"),
  },
};
