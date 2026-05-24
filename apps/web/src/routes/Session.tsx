import { useParams } from "react-router-dom";
import { MainLayout } from "@/components/layout/MainLayout";
import { TurnCard } from "@/components/chat/TurnCard";
import { FileAccessBar } from "@/components/chat/FileAccessBar";
import { TextContent } from "@/components/chat/TextContent";
import { ToolCard } from "@/components/chat/ToolCard";
import { Composer } from "@/components/chat/Composer";
import { RightPanel } from "@/components/right-panel/RightPanel";
import { DiffViewer } from "@/components/right-panel/DiffViewer";
import { TerminalPanel } from "@/components/right-panel/TerminalPanel";
import { StatusPanel } from "@/components/right-panel/StatusPanel";

const introMarkdown = [
  "## Hello! I'm kuku",
  "",
  "I can help you with:",
  "",
  "- Reading and writing files",
  "- Running shell commands",
  "- Managing sub-agents for complex tasks",
  "",
  "How can I help you today?",
].join("\n");

export function Session() {
  const { id } = useParams<{ id: string }>();

  return (
    <MainLayout
      sessionTitle={id ?? "unknown"}
      sessionStatus="idle"
      rightPanel={
        <RightPanel
          diffContent={<DiffViewer />}
          terminalContent={<TerminalPanel />}
          statusContent={<StatusPanel />}
        />
      }
    >
      <div className="flex flex-col flex-1 min-h-0">
        <div className="flex-1 overflow-auto px-4 py-4 space-y-3">
          <TurnCard role="user">
            <p>Help me set up the project.</p>
          </TurnCard>
          <TurnCard role="agent">
            <FileAccessBar />
            <TextContent text={introMarkdown} />
          </TurnCard>
          <TurnCard role="user">
            <p>Show me the gateway configuration file.</p>
          </TurnCard>
          <TurnCard role="agent">
            <ToolCard
              icon="👁️"
              name="read_file"
              summary="src/api/gateway.ts (142 lines)"
              status="completed"
            >
              <pre className="font-mono text-[var(--text-xs)] text-[var(--color-text-secondary)] p-2 bg-[var(--color-surface)] rounded-[var(--radius-sm)] border border-[var(--color-border)] overflow-x-auto">
{`export class Gateway {
  private rateLimiter: RateLimiter;
  constructor(config: GatewayConfig) {
    this.rateLimiter = new RateLimiter(config.limit);
  }
}`}
              </pre>
            </ToolCard>
            <TextContent text="The gateway is configured with rate limiting at 1000 req/s." />
          </TurnCard>
        </div>
        <Composer onSubmit={() => {}} />
      </div>
    </MainLayout>
  );
}
