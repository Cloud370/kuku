import { useParams, useNavigate } from "react-router-dom";
import { useEffect, useCallback } from "react";
import { MainLayout } from "@/components/layout/MainLayout";
import { TurnCard } from "@/components/chat/TurnCard";
import { TextContent } from "@/components/chat/TextContent";
import { ToolCard } from "@/components/chat/ToolCard";
import { ThinkingBlock } from "@/components/chat/ThinkingBlock";
import { ErrorNotice } from "@/components/chat/ErrorNotice";
import { CancelledNotice } from "@/components/chat/CancelledNotice";
import { PermissionDock } from "@/components/chat/PermissionDock";
import { Composer } from "@/components/chat/Composer";
import { RightPanel } from "@/components/right-panel/RightPanel";
import { DiffViewer } from "@/components/right-panel/DiffViewer";
import { TerminalPanel } from "@/components/right-panel/TerminalPanel";
import { StatusPanel } from "@/components/right-panel/StatusPanel";
import { useUIStore } from "@/stores/ui";
import { useRunStore } from "@/stores/run";
import { useSessionEvents } from "@/queries/sessions";
import { replayToTurns } from "@/adapters/replay";
import type { EventPayload } from "@/adapters/replay";
import { createRun } from "@/api/runs";

function riskIcon(risk: string): string {
  if (risk === "command") return ">";
  if (risk === "file_write") return "+";
  if (risk === "file_read") return "\u{1F441}";
  return "\u{1F527}";
}

export function Session() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const workspace = useUIStore((s) => s.workspace);
  const { turns, status, pendingPermission, loadTurns, pushWireLine, pushActiveStream, setStatus, clear, addUserTurn, respondToPermission } = useRunStore();
  const isNew = !id || id === "new";

  const { data } = useSessionEvents(isNew ? undefined : id, workspace);

  useEffect(() => {
    clear();
  }, [id, clear]);

  useEffect(() => {
    if (!data) return;
    if (Array.isArray(data)) {
      loadTurns(replayToTurns(data.map((e) => e.payload as EventPayload)));
    } else {
      loadTurns(replayToTurns(data.events.map((e) => e.payload as EventPayload)));
      if (data.active_stream?.length) {
        pushActiveStream(data.active_stream);
      }
    }
  }, [data, loadTurns, pushActiveStream]);

  const handleSubmit = useCallback(
    (prompt: string, _model: string) => {
      setStatus("streaming");
      addUserTurn(prompt);
      void createRun(
        prompt,
        workspace,
        isNew ? undefined : id,
        (line) => {
          pushWireLine(line);
        },
        (sessionId) => {
          if (isNew) void navigate(`/session/${sessionId}`, { replace: true });
        },
        () => {
          setStatus("error");
        },
      );
    },
    [workspace, id, isNew, pushWireLine, setStatus, addUserTurn, navigate],
  );

  const isLoading = !isNew && status === "idle";
  if (isLoading) {
    return (
      <div className="h-screen flex items-center justify-center text-[var(--color-text-muted)]">
        Loading...
      </div>
    );
  }

  return (
    <MainLayout
      sessionTitle={id ?? "New Session"}
      sessionStatus={status === "streaming" ? "running" : "idle"}
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
          {turns.length === 0 && isNew ? (
            <div className="text-center text-[var(--color-text-muted)] mt-20">
              Start a conversation.
            </div>
          ) : (
            turns.map((turn) => (
              <div key={turn.turnNumber} className="space-y-3">
                <TurnCard role="user">
                  <p>{turn.userText}</p>
                </TurnCard>
                <TurnCard role="agent">
                  {turn.agent.error && (
                    <ErrorNotice
                      type="internal"
                      message={turn.agent.error.message}
                    />
                  )}
                  {turn.status === "cancelled" && (
                    <CancelledNotice turnNumber={turn.turnNumber} />
                  )}
                  {turn.agent.thinking && (
                    <ThinkingBlock>{turn.agent.thinking}</ThinkingBlock>
                  )}
                  {turn.agent.text && (
                    <TextContent text={turn.agent.text} />
                  )}
                  {turn.agent.tools.map((t) => {
                    const hasSub = t.subEvents.length > 0 || t.modelContent;
                    return (
                      <ToolCard
                        key={t.id}
                        name={t.name}
                        summary={t.summary}
                        status={
                          t.status === "running"
                            ? "running"
                            : t.status === "error"
                              ? "error"
                              : "completed"
                        }
                        kind={t.kind === "agent" ? "agent" : "tool"}
                        childSessionId={t.childSessionId}
                      >
                        {hasSub ? (
                          <div className="space-y-1">
                            {t.modelContent ? (
                              <pre className="text-xs text-[var(--color-text-secondary)] whitespace-pre-wrap">
                                {t.modelContent}
                              </pre>
                            ) : null}
                            {t.subEvents.map((se, i) => {
                              if (se.type === "stdout")
                                return <pre key={i} className="font-mono text-xs text-[var(--color-text-primary)] whitespace-pre-wrap">{se.text}</pre>;
                              if (se.type === "stderr")
                                return <pre key={i} className="font-mono text-xs text-red-400 whitespace-pre-wrap">{se.text}</pre>;
                              if (se.type === "thinking")
                                return <p key={i} className="text-xs text-[var(--color-text-muted)] whitespace-pre-wrap">{se.content}</p>;
                              if (se.type === "text")
                                return <p key={i} className="text-sm text-[var(--color-text-secondary)] whitespace-pre-wrap">{se.content}</p>;
                              return null;
                            })}
                          </div>
                        ) : undefined}
                      </ToolCard>
                    );
                  })}
                </TurnCard>
              </div>
            ))
          )}
        </div>
        {pendingPermission && (
            <PermissionDock
              toolIcon={riskIcon(pendingPermission.risk)}
              toolName={pendingPermission.tool}
              riskLabel={pendingPermission.risk}
              summary={pendingPermission.summary}
              onDeny={() => { void respondToPermission(id ?? "", pendingPermission.id, "deny"); }}
              onAllowOnce={() => { void respondToPermission(id ?? "", pendingPermission.id, "once"); }}
              onAllowAlways={() => { void respondToPermission(id ?? "", pendingPermission.id, "project"); }}
            />
          )}
          <Composer onSubmit={handleSubmit} />
      </div>
    </MainLayout>
  );
}
