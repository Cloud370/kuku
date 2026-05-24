import { useNavigate } from "react-router-dom";
import { Header } from "@/components/layout/Header";
import { SessionList } from "@/components/layout/SessionList";
import { useUIStore } from "@/stores/ui";
import type { LayoutMode } from "@/stores/ui";
import { useSessions } from "@/queries/sessions";
import type { SessionSummary } from "@/api/sessions";
import type { SessionListProps } from "@/components/layout/SessionList";

type SessionItem = NonNullable<SessionListProps["groups"]>[number]["sessions"][number];
type SessionGroup = NonNullable<SessionListProps["groups"]>[number];

function toGroups(sessions: SessionSummary[]): SessionGroup[] {
  const today = new Date().toDateString();
  const map = new Map<string, SessionItem[]>();
  for (const s of sessions) {
    const date = new Date(s.mtime).toDateString();
    const label = date === today ? "Today" : new Date(s.mtime).toLocaleDateString();
    const list = map.get(label) ?? [];
    list.push({
      id: s.session_id,
      preview: s.title || s.session_id,
      status: mapStatus(s.status),
    });
    map.set(label, list);
  }
  return [...map.entries()].map(([label, sessions]) => ({ label, sessions }));
}

function mapStatus(s: string): SessionItem["status"] {
  if (s === "Active") return "running";
  if (s === "Interrupted") return "waiting";
  return "completed";
}

export function Home() {
  const navigate = useNavigate();
  const layoutMode = useUIStore((s) => s.layoutMode);
  const setLayoutMode = useUIStore((s) => s.setLayoutMode);
  const workspace = useUIStore((s) => s.workspace);

  const { data, isLoading, error } = useSessions(workspace);
  const groups = data?.sessions ? toGroups(data.sessions) : [];

  return (
    <div className="h-screen flex flex-col bg-[var(--color-surface)]">
      <Header
        sessionTitle="kuku"
        layoutMode={layoutMode}
        onLayoutModeChange={(m: LayoutMode) => { setLayoutMode(m); }}
      />
      <div className="flex-1 min-h-0 max-w-2xl mx-auto w-full">
        {isLoading ? (
          <div className="flex items-center justify-center h-full text-[var(--color-text-muted)]">
            Loading sessions...
          </div>
        ) : error ? (
          <div className="flex items-center justify-center h-full text-red-400">
            Failed to load sessions
          </div>
        ) : (
          <SessionList
            groups={groups}
            onSelect={(id) => { void navigate(`/session/${id}`); }}
            onNew={() => { void navigate("/session/new"); }}
          />
        )}
      </div>
    </div>
  );
}
