import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useLocation, useNavigate } from "react-router-dom";
import { errorMessage } from "@/api/client";
import type { Intent } from "@/api/sessions";
import { Button } from "@/components/ui/Button";
import { sessionHref } from "@/routes/navigation";
import { useCommands } from "@/stores/commands";
import { targetKey, useDrafts } from "@/stores/drafts";

function describe(intent: Intent) {
  const command = intent.command;
  switch (command.type) {
    case "send":
      return command.text;
    case "reply_question":
      return command.text;
    case "reply_permission":
      return command.allow ? "Allow once" : "Deny";
    case "create_agent":
      return "Create Agent";
    case "configure":
      return "Change model settings";
    case "withdraw":
      return "Withdraw input";
    case "advance":
      return "Continue";
    case "cancel":
      return "Stop execution";
  }
}

export function CommandRecovery() {
  const pending = useCommands((s) => s.pending);
  const outcome = useCommands((s) => s.outcome);
  const restore = useCommands((s) => s.restore);
  const retry = useCommands((s) => s.retry);
  const check = useCommands((s) => s.check);
  const discard = useCommands((s) => s.discard);
  const dismissOutcome = useCommands((s) => s.dismissOutcome);
  const navigate = useNavigate();
  const location = useLocation();
  const client = useQueryClient();
  const [error, setError] = useState<string>();
  const handledCreation = useRef<string>(undefined);

  useEffect(restore, [restore]);
  useEffect(() => {
    if (!outcome) return;
    void client.invalidateQueries({ queryKey: ["sessions", outcome.intent.workspace] });
  }, [client, outcome]);
  useEffect(() => {
    if (outcome?.intent.command.type !== "send" || outcome.intent.command.create === undefined)
      return;
    if (handledCreation.current === outcome.intent.commandId) return;
    handledCreation.current = outcome.intent.commandId;
    const workDir = new URLSearchParams(location.search).get("workspace");
    if (location.pathname === "/session/new" && workDir === outcome.intent.workspace) {
      void navigate(sessionHref(outcome.intent), { replace: true });
    }
  }, [location.pathname, location.search, navigate, outcome]);

  function discardIntent(intent: Intent) {
    try {
      discard(intent.commandId);
      if (intent.command.type === "send") {
        const key = targetKey(
          intent.command.create === undefined ? intent : { ...intent, session: "" },
        );
        const { drafts, change } = useDrafts.getState();
        if (!drafts[key]) change(key, intent.command.text);
      }
      setError(undefined);
    } catch (error) {
      setError(errorMessage(error));
    }
  }

  const details = outcome?.result.status;
  const confirmedTarget = outcome?.intent;
  const confirmedCreate =
    confirmedTarget?.command.type === "send" && confirmedTarget.command.create !== undefined;
  if (pending.length === 0 && !error && !details && !confirmedCreate) return null;

  return (
    <div className="max-h-44 overflow-auto shrink-0 border-b border-[var(--color-border)] bg-[var(--color-surface-raised)]">
      {error && (
        <p role="alert" className="px-4 py-2 text-xs text-red-400">
          {error}
        </p>
      )}
      {pending.map(({ intent, status, message }) => (
        <div
          key={intent.commandId}
          className="px-4 py-2 flex gap-3 items-center justify-between text-sm"
        >
          <div className="min-w-0">
            <p role="status" className="text-xs text-[var(--color-text-muted)]">
              {status === "sending"
                ? "Sending…"
                : status === "missing"
                  ? "No receipt found"
                  : status === "unknown"
                    ? "Receipt unavailable"
                    : status === "conflict"
                      ? "Command conflict"
                      : "Checking receipt…"}
              {` · ${intent.agent} · ${intent.session} · ${intent.workspace}`}
            </p>
            <p className="truncate" title={describe(intent)}>
              {describe(intent)}
            </p>
            {message && <p className="text-xs text-red-400">{message}</p>}
          </div>
          <div className="flex gap-2 shrink-0">
            {status === "missing" && (
              <Button
                variant="secondary"
                size="sm"
                onClick={() => {
                  void retry(intent.commandId);
                }}
              >
                Resend original
              </Button>
            )}
            {(status === "missing" || status === "unknown") && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  void check(intent.commandId);
                }}
              >
                Check receipt
              </Button>
            )}
            {(status === "missing" || status === "conflict") && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  discardIntent(intent);
                }}
              >
                Discard
              </Button>
            )}
          </div>
        </div>
      ))}
      {confirmedTarget && (details || confirmedCreate) && (
        <div className="px-4 py-1 flex gap-3 items-center justify-between text-xs">
          <p role="status">
            {details === "queued"
              ? "Queue receipt confirmed"
              : details === "accepted"
                ? "Input accepted"
                : details === "withdrawn"
                  ? "Input withdrawn"
                  : "Session created"}
            {` · ${confirmedTarget.agent}`}
          </p>
          <div className="flex items-center gap-3">
            <Link
              className="text-[var(--color-accent)] underline"
              to={sessionHref(confirmedTarget)}
            >
              Open session
            </Link>
            <Button variant="ghost" size="sm" onClick={dismissOutcome}>
              Dismiss
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
