import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { SubmitEvent, ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";
import { errorMessage } from "@/api/client";
import type { AgentStatus, Command, ModelRef, SendTiming, Target } from "@/api/sessions";
import { Composer } from "@/components/chat/Composer";
import { ConversationTimeline } from "@/components/chat/ConversationTimeline";
import { ErrorNotice } from "@/components/chat/ErrorNotice";
import { ModelControl } from "@/components/chat/ModelControl";
import { PendingQuestion } from "@/components/chat/PendingQuestion";
import { PermissionDock } from "@/components/chat/PermissionDock";
import { LeftSidebar } from "@/components/layout/LeftSidebar";
import { MainLayout } from "@/components/layout/MainLayout";
import { SessionBrowser } from "@/components/layout/SessionBrowser";
import { DiffViewer } from "@/components/right-panel/DiffViewer";
import type { DiffViewerProps } from "@/components/right-panel/DiffViewer";
import { RightPanel } from "@/components/right-panel/RightPanel";
import { StatusPanel } from "@/components/right-panel/StatusPanel";
import { TerminalPanel } from "@/components/right-panel/TerminalPanel";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";
import { visibleOutput } from "@/lib/output";
import { useConversation } from "@/queries/conversation";
import { useAgents } from "@/queries/sessions";
import { useCommands } from "@/stores/commands";
import { targetKey, useDrafts } from "@/stores/drafts";
import { sessionHref, workspaceHref } from "./navigation";

const statusLabels: Record<AgentStatus, string> = {
  idle: "Idle",
  running: "Running",
  waiting: "Waiting for reply",
  stopped: "Stopped",
  interrupted: "Interrupted",
};

function diffLines(diff: string | undefined): NonNullable<DiffViewerProps["lines"]> {
  return (
    diff?.split("\n").map((line) => ({
      content: visibleOutput(line),
      type:
        line.startsWith("+") && !line.startsWith("+++")
          ? "added"
          : line.startsWith("-") && !line.startsWith("---")
            ? "removed"
            : "unchanged",
    })) ?? []
  );
}

export function Session({ target, workspacePanel }: { target: Target; workspacePanel: ReactNode }) {
  const { workspace, session, agent } = target;
  const isNew = session === "";
  const key = targetKey(target);
  const draft = useDrafts((s) => s.drafts[key] ?? "");
  const changeDraft = useDrafts((s) => s.change);
  const pending = useCommands((s) => s.pending);
  const submit = useCommands((s) => s.submit);
  const navigate = useNavigate();
  const [draftModel, setDraftModel] = useState<ModelRef>();
  const [error, setError] = useState<string>();
  const [creatingAgent, setCreatingAgent] = useState(false);
  const viewport = useRef<HTMLDivElement>(null);
  const followLatest = useRef(true);
  const scrollAnchor = useRef<number>(undefined);
  const observationTarget = useMemo(
    () => (session ? { workspace, session, agent } : undefined),
    [workspace, session, agent],
  );
  const { view, loadOlder, refresh, reconnect } = useConversation(observationTarget);
  const fallback = useAgents(target, !view.snapshot && view.errorCode === "agent_not_found");
  const snapshot = view.snapshot;
  const agents = snapshot?.agents ?? fallback.data?.agents ?? [];
  const status = snapshot?.agent.status;
  const execution = agents.find(({ address }) => address === agent)?.execution_id;
  const question = snapshot?.agent.question;
  const permission = snapshot?.agent.permission;
  const busy = status === "running" || status === "waiting";
  const cleaning = snapshot?.cleaning_up === true;
  const enabled = !!snapshot && view.connection === "connected";
  const lastDiff = view.entries.findLast(
    (entry) => entry.type === "tool_result" && typeof entry.structured?.diff === "string",
  );
  const diff =
    lastDiff?.type === "tool_result" && typeof lastDiff.structured?.diff === "string"
      ? lastDiff.structured.diff
      : undefined;
  const unconfirmedCreate = pending.some(
    ({ intent }) =>
      intent.workspace === workspace &&
      intent.command.type === "send" &&
      intent.command.create !== undefined,
  );

  function pendingAction(type: Command["type"], requestId?: string) {
    return pending.some(
      ({ intent }) =>
        intent.session === session &&
        intent.workspace === workspace &&
        intent.agent === agent &&
        intent.command.type === type &&
        (requestId === undefined ||
          ("request_id" in intent.command && intent.command.request_id === requestId)),
    );
  }

  useLayoutEffect(() => {
    const panel = viewport.current;
    if (!panel) return;
    if (scrollAnchor.current !== undefined) {
      panel.scrollTop = panel.scrollHeight - scrollAnchor.current;
      scrollAnchor.current = undefined;
    } else if (followLatest.current) {
      panel.scrollTop = panel.scrollHeight;
    }
  }, [view.entries, view.preview]);

  async function dispatch(command: Command, to = target, onSaved?: () => void) {
    setError(undefined);
    try {
      await submit({ ...to, command, commandId: crypto.randomUUID() }, onSaved);
      refresh();
    } catch (error) {
      setError(errorMessage(error));
    }
  }

  function send(text: string, timing: SendTiming) {
    if (isNew) {
      if (unconfirmedCreate) return;
      void dispatch(
        { type: "send", text, timing, create: draftModel ?? {} },
        { ...target, session: `s-${crypto.randomUUID()}` },
        () => {
          changeDraft(key, "");
        },
      );
    } else {
      void dispatch({ type: "send", text, timing }, target, () => {
        changeDraft(key, "");
      });
    }
  }

  function changeModel(model?: ModelRef) {
    if (!snapshot) {
      setDraftModel(model);
      return;
    }
    if (!model) return;
    const config = {
      ...snapshot.agent.config,
      ...model,
      reasoning_effort: null,
      service_tier: null,
      output: null,
      max_output_tokens: null,
    };
    void dispatch({ type: "configure", config });
  }

  function createAgent(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    const address = new FormData(event.currentTarget).get("address");
    if (typeof address !== "string" || !address) return;
    void dispatch({ type: "create_agent", config: {} }, { ...target, agent: address }, () => {
      setCreatingAgent(false);
    });
  }

  const context = {
    model: snapshot?.selection?.model,
    provider: snapshot?.selection?.connection,
    turn: snapshot?.agent.turn,
    tokens: snapshot?.agent.usage,
    sessionId: session,
    workspace: snapshot?.agent.config.workspace ?? workspace,
    agent,
  };
  const executionError = snapshot?.agent.last_error;
  const lastErrorVisible = view.entries.some(
    (entry) => entry.type === "error" && entry.text === executionError,
  );

  return (
    <MainLayout
      sessionTitle={session || "New Session"}
      sessionStatus={status === "running" || status === "waiting" ? status : "idle"}
      leftPanel={
        <LeftSidebar
          workspacePanel={workspacePanel}
          sessions={<SessionBrowser workspace={workspace} activeId={session} />}
        />
      }
      rightPanel={
        <RightPanel
          diffContent={<DiffViewer lines={diffLines(diff)} />}
          terminalContent={
            <TerminalPanel
              lines={view.preview?.tools.flatMap(({ lines }) => lines).slice(-50) ?? []}
            />
          }
          statusContent={
            <StatusPanel
              context={context}
              events={view.entries.slice(-50).map(({ id, type }) => `#${String(id)} ${type}`)}
            />
          }
        />
      }
    >
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex flex-wrap gap-2 shrink-0 items-center px-4 py-2 border-b border-[var(--color-border)] text-xs">
          <Link to={workspaceHref(workspace)} className="text-[var(--color-accent)] underline">
            Sessions
          </Link>
          <span title={workspace} className="min-w-0 truncate text-[var(--color-text-muted)]">
            {workspace}
          </span>
          {agents.length > 0 && (
            <select
              aria-label="Agent"
              value={agent}
              className="rounded border border-[var(--color-border)] bg-[var(--color-surface-raised)] px-1.5 py-1"
              onChange={(event) => {
                void navigate(sessionHref({ ...target, agent: event.target.value }));
              }}
            >
              {!agents.some(({ address }) => address === agent) && (
                <option value={agent} disabled>
                  {agent} (unavailable)
                </option>
              )}
              {agents.map(({ address, status }) => (
                <option key={address} value={address}>
                  {address} ({statusLabels[status]})
                </option>
              ))}
            </select>
          )}
          {status && (
            <span role="status" className="font-medium">
              {cleaning ? "Stopping…" : statusLabels[status]}
            </span>
          )}
          {execution != null && busy && (
            <Button
              size="sm"
              variant="danger"
              disabled={!enabled || pendingAction("cancel")}
              onClick={() => {
                void dispatch({ type: "cancel", execution_id: execution });
              }}
            >
              Stop
            </Button>
          )}
          {(status === "stopped" ||
            status === "interrupted" ||
            (status === "idle" && !!snapshot?.pending.length)) && (
            <Button
              size="sm"
              variant="secondary"
              disabled={!enabled || cleaning || pendingAction("advance")}
              onClick={() => {
                void dispatch({ type: "advance" });
              }}
            >
              Continue
            </Button>
          )}
          {enabled && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setCreatingAgent(!creatingAgent);
              }}
            >
              {creatingAgent ? "Cancel" : "New agent"}
            </Button>
          )}
          {creatingAgent && enabled && (
            <form onSubmit={createAgent} className="flex gap-1 items-center">
              <Input
                name="address"
                aria-label="Agent address"
                placeholder="reviewer"
                pattern={"[a-z0-9_\\-]{1,64}"}
                required
              />
              <Button type="submit" size="sm">
                Create agent
              </Button>
            </form>
          )}
        </div>
        {!isNew && view.connection !== "connected" && (
          <div className="px-4 py-2 flex items-center gap-3 text-xs" role="status">
            {view.connection === "connecting"
              ? "Loading conversation…"
              : view.connection === "reconnecting"
                ? "Reconnecting…"
                : "Conversation unavailable"}
            {view.error && <span className="text-red-400">{view.error}</span>}
            {view.connection === "failed" && (
              <Button variant="secondary" size="sm" onClick={reconnect}>
                Retry connection
              </Button>
            )}
          </div>
        )}
        {view.connection === "connected" && view.error && (
          <div className="px-4 py-2 flex items-center gap-3 text-xs" role="alert">
            <span className="text-red-400">{view.error}</span>
            <Button variant="secondary" size="sm" onClick={refresh}>
              Refresh status
            </Button>
          </div>
        )}
        <div
          ref={viewport}
          className="flex-1 overflow-auto px-4 py-4 space-y-3"
          onScroll={(event) => {
            const panel = event.currentTarget;
            followLatest.current = panel.scrollHeight - panel.scrollTop - panel.clientHeight < 48;
          }}
        >
          {view.before !== null && (
            <div className="text-center">
              <Button
                size="sm"
                variant="secondary"
                disabled={view.loadingOlder}
                onClick={() => {
                  loadOlder(() => {
                    const panel = viewport.current;
                    if (panel) scrollAnchor.current = panel.scrollHeight - panel.scrollTop;
                  });
                }}
              >
                {view.loadingOlder ? "Loading earlier messages…" : "Load earlier messages"}
              </Button>
              {view.historyError && (
                <p role="alert" className="text-xs text-red-400">
                  {view.historyError}
                </p>
              )}
            </div>
          )}
          <ConversationTimeline target={target} view={view} />
          {executionError && !lastErrorVisible && (
            <ErrorNotice type="server" message={executionError} />
          )}
        </div>
        {!!snapshot?.pending.length && (
          <div className="max-h-32 overflow-auto shrink-0 border-t border-[var(--color-border)] px-4 py-2">
            <p className="text-xs font-medium">Queued for {agent}</p>
            <ul>
              {snapshot.pending.map(({ receipt, input }) => (
                <li
                  key={receipt.command_id}
                  className="flex items-center justify-between gap-2 text-sm"
                >
                  <span className="truncate" title={input.text}>
                    {input.text}
                  </span>
                  <span className="text-xs text-[var(--color-text-muted)] ml-auto">
                    {input.timing === "next_turn" ? "Next turn" : "Next step"}
                  </span>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={!enabled || pendingAction("withdraw")}
                    onClick={() => {
                      void dispatch({ type: "withdraw", input_command_id: receipt.command_id });
                    }}
                  >
                    Withdraw
                  </Button>
                </li>
              ))}
            </ul>
          </div>
        )}
        {question && (
          <PendingQuestion
            key={question.id}
            request={question}
            agent={agent}
            disabled={!enabled || pendingAction("reply_question", question.id)}
            onAnswer={(text) => {
              void dispatch({ type: "reply_question", request_id: question.id, text });
            }}
          />
        )}
        {permission && (
          <PermissionDock
            toolName={`${agent} · ${permission.tool}`}
            riskLabel={permission.risk}
            requestId={permission.id}
            summary={permission.summary}
            detail={
              permission.candidate && permission.candidate !== permission.summary
                ? visibleOutput(permission.candidate)
                : undefined
            }
            disabled={!enabled || pendingAction("reply_permission", permission.id)}
            onDeny={() => {
              void dispatch({ type: "reply_permission", request_id: permission.id, allow: false });
            }}
            onAllowOnce={() => {
              void dispatch({ type: "reply_permission", request_id: permission.id, allow: true });
            }}
          />
        )}
        <Composer
          agent={agent}
          busy={busy}
          value={draft}
          onChange={(text) => {
            changeDraft(key, text);
          }}
          onSubmit={send}
          disabled={isNew ? unconfirmedCreate : !enabled || cleaning}
          error={
            error ??
            snapshot?.pending_selection_error?.message ??
            snapshot?.selection_error?.message
          }
          onDismissError={
            error
              ? () => {
                  setError(undefined);
                }
              : undefined
          }
          modelControl={
            <ModelControl
              snapshot={snapshot}
              draftModel={draftModel}
              onChange={changeModel}
              disabled={!isNew && (!enabled || pendingAction("configure"))}
            />
          }
        />
      </div>
    </MainLayout>
  );
}
