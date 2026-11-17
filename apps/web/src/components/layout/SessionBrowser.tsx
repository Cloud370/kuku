import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { errorMessage } from "@/api/client";
import type { SessionSummary } from "@/api/sessions";
import { useSessions } from "@/queries/sessions";
import { draftHref, sessionHref } from "@/routes/navigation";
import { Input } from "@/components/ui/Input";
import { SessionList } from "./SessionList";
import type { SessionListProps } from "./SessionList";

type SessionGroup = NonNullable<SessionListProps["groups"]>[number];

function sessionGroups(sessions: SessionSummary[]): SessionGroup[] {
  const today = new Date().toDateString();
  const groups = new Map<string, SessionGroup["sessions"]>();
  for (const session of sessions) {
    const date = new Date(session.mtime);
    const label = date.toDateString() === today ? "Today" : date.toLocaleDateString();
    const items = groups.get(label) ?? [];
    const status = session.status.toLowerCase();
    items.push({
      id: session.session_id,
      preview: session.title || session.session_id,
      status:
        session.waiting_agents?.length || ["waiting", "interrupted"].includes(status)
          ? "waiting"
          : status === "active" || status === "running"
            ? "running"
            : "completed",
    });
    groups.set(label, items);
  }
  return [...groups].map(([label, sessions]) => ({ label, sessions }));
}

export function SessionBrowser({ workspace, activeId }: { workspace: string; activeId?: string }) {
  const navigate = useNavigate();
  const [search, setSearch] = useState("");
  const sessions = useSessions(workspace, search);
  if (!workspace) return null;
  return (
    <div className="h-full flex flex-col overflow-hidden">
      <div className="p-2 shrink-0">
        <Input
          aria-label="Search sessions"
          placeholder="Search sessions"
          value={search}
          onChange={(event) => {
            setSearch(event.target.value);
          }}
        />
      </div>
      {sessions.isLoading && (
        <p className="p-3 text-xs text-[var(--color-text-muted)]">Loading sessions…</p>
      )}
      {sessions.error && (
        <p role="alert" className="p-3 text-xs text-red-400">
          {errorMessage(sessions.error)}
        </p>
      )}
      <SessionList
        groups={sessionGroups(sessions.data?.sessions ?? [])}
        activeId={activeId}
        onSelect={(session) => {
          void navigate(sessionHref({ workspace, session, agent: "main" }));
        }}
        onNew={() => {
          void navigate(draftHref(workspace));
        }}
      />
    </div>
  );
}
