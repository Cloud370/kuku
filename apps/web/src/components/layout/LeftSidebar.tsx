import { useNavigate } from "react-router-dom";
import { Panel, Group, Separator } from "react-resizable-panels";
import { useUIStore } from "@/stores/ui";
import { useSessions } from "@/queries/sessions";
import type { SessionSummary } from "@/api/sessions";
import type { SessionListProps } from "./SessionList";
import { WorkspaceZone } from "./WorkspaceZone";
import { SessionList } from "./SessionList";

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

const vertSep =
  "group flex items-center justify-center h-[6px] shrink-0 hover:bg-[var(--color-accent-muted)] transition-colors";
const vertLine =
  "h-[1px] w-1/3 bg-[var(--color-border)] group-hover:bg-[var(--color-accent)] transition-colors";

export function LeftSidebar() {
  const theme = useUIStore((s) => s.theme);
  const toggleTheme = useUIStore((s) => s.toggleTheme);
  const workspace = useUIStore((s) => s.workspace);
  const navigate = useNavigate();

  const { data, isLoading, error } = useSessions(workspace);
  const groups = data?.sessions ? toGroups(data.sessions) : [];

  return (
    <aside className="h-full flex flex-col bg-[var(--color-surface)] border-r border-[var(--color-border)]">
      <Group orientation="vertical" className="flex-1 min-h-0">
        <Panel defaultSize={33} minSize="80px" maxSize="50%">
          <WorkspaceZone />
        </Panel>
        <Separator className={vertSep}>
          <div className={vertLine} />
        </Separator>
        <Panel defaultSize={67} minSize="120px">
          {isLoading ? (
            <div className="p-3 text-[var(--text-xs)] text-[var(--color-text-muted)]">
              Loading sessions...
            </div>
          ) : error ? (
            <div className="p-3 text-[var(--text-xs)] text-red-400">
              Failed to load sessions
            </div>
          ) : (
            <SessionList
              groups={groups}
              onSelect={(id) => { void navigate(`/session/${id}`); }}
              onNew={() => { void navigate("/session/new"); }}
            />
          )}
        </Panel>
      </Group>
      <div className="border-t border-[var(--color-border)] p-2 flex justify-center shrink-0">
        <button
          onClick={toggleTheme}
          className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] text-[var(--text-lg)] cursor-pointer select-none transition-colors p-1 rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)]"
          aria-label="Toggle theme"
        >
          {theme === "dark" ? "☀" : "☽"}
        </button>
      </div>
    </aside>
  );
}
