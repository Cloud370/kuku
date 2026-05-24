import { cn } from "@/lib/cn";

type SessionStatus = "running" | "waiting" | "completed" | "error";

interface SessionItem {
  id: string;
  preview: string;
  status: SessionStatus;
}

interface SessionGroup {
  label: string;
  sessions: SessionItem[];
}

const statusDot: Record<SessionStatus, string> = {
  running: "bg-green-400",
  waiting: "bg-yellow-400",
  completed: "bg-[var(--color-text-muted)]",
  error: "bg-red-400",
};

const exampleGroups: SessionGroup[] = [
  {
    label: "Today",
    sessions: [
      { id: "sess-a3f2", preview: "How do I configure the API gateway for...", status: "running" },
      { id: "sess-7b1d", preview: "Refactor the auth middleware to use...", status: "waiting" },
    ],
  },
  {
    label: "Yesterday",
    sessions: [
      { id: "sess-c9e8", preview: "Add integration tests for the payment flow", status: "completed" },
      { id: "sess-2f4a", preview: "Fix memory leak in WebSocket handler", status: "completed" },
      { id: "sess-d5b6", preview: "Debug CI pipeline timeout on Windows...", status: "error" },
    ],
  },
  {
    label: "Older",
    sessions: [
      { id: "sess-8a1c", preview: "Update dependencies to latest versions", status: "completed" },
      { id: "sess-e3f0", preview: "Initial project scaffold and setup", status: "completed" },
    ],
  },
];

export type SessionListProps = {
  groups?: SessionGroup[];
  activeId?: string;
  onSelect?: (id: string) => void;
};

export function SessionList({ groups = exampleGroups, activeId, onSelect }: SessionListProps) {
  return (
    <div className="h-full flex flex-col overflow-auto">
      <div className="flex items-center justify-between px-3 pt-3 pb-2 shrink-0">
        <p className="text-[var(--text-xs)] font-medium text-[var(--color-text-muted)] uppercase tracking-wider">
          Sessions
        </p>
        <button
          className="text-[var(--text-xs)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] p-1 rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] transition-colors cursor-pointer"
          aria-label="New session"
          title="New session"
        >
          +
        </button>
      </div>
      <div className="flex-1 flex flex-col gap-0.5 px-2 pb-2">
        {groups.map((group) => (
          <div key={group.label} className="mb-1">
            <p className="text-[var(--text-xs)] text-[var(--color-text-muted)] px-2 py-1 font-medium">
              {group.label}
            </p>
            {group.sessions.map((s) => (
              <button
                key={s.id}
                onClick={() => onSelect?.(s.id)}
                className={cn(
                  "w-full text-left px-2 py-1.5 rounded-[var(--radius-md)] transition-colors cursor-pointer group",
                  s.id === activeId
                    ? "bg-[var(--color-accent-muted)]"
                    : "hover:bg-[var(--color-surface-hover)]",
                )}
              >
                <div className="flex items-center gap-2">
                  <span
                    className={cn("w-2 h-2 rounded-full shrink-0", statusDot[s.status])}
                  />
                  <span className="text-[var(--text-sm)] text-[var(--color-text-primary)] font-medium">
                    {s.id}
                  </span>
                </div>
                <p className="text-[var(--text-xs)] text-[var(--color-text-muted)] truncate mt-0.5 ml-4">
                  {s.preview}
                </p>
              </button>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}
