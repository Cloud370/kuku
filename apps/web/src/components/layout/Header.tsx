import type { LayoutMode } from "@/stores/ui";

export type HeaderProps = {
  sessionTitle?: string;
  sessionStatus?: "running" | "waiting" | "completed" | "idle";
  layoutMode?: LayoutMode;
  onLayoutModeChange?: (mode: LayoutMode) => void;
};

const statusColors: Record<string, string> = {
  running: "bg-green-400",
  waiting: "bg-yellow-400",
  completed: "bg-[var(--color-text-muted)]",
  idle: "bg-[var(--color-text-muted)]",
};

const nextLayout: Record<LayoutMode, LayoutMode> = {
  "one-column": "two-column",
  "two-column": "three-column",
  "three-column": "one-column",
};

const layoutLabel: Record<LayoutMode, string> = {
  "one-column": "1-Col",
  "two-column": "2-Col",
  "three-column": "3-Col",
};

export function Header({
  sessionTitle = "New Session",
  sessionStatus = "idle",
  layoutMode = "three-column",
  onLayoutModeChange = () => {},
}: HeaderProps) {
  return (
    <header className="flex items-center justify-between h-[44px] px-4 border-b border-[var(--color-border)] bg-[var(--color-surface-raised)] shrink-0">
      <div className="flex items-center gap-2">
        <span className="text-[var(--text-sm)] font-medium text-[var(--color-text-primary)]">
          {sessionTitle}
        </span>
        <span
          className={`inline-block w-2 h-2 rounded-full ${statusColors[sessionStatus]}`}
        />
      </div>
      <div className="flex items-center gap-2">
        <button
          onClick={() => onLayoutModeChange(nextLayout[layoutMode])}
          className="text-[var(--text-xs)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] px-2 py-1 rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] transition-colors cursor-pointer"
          aria-label={`Layout mode: ${layoutLabel[layoutMode]}`}
        >
          {layoutLabel[layoutMode]}
        </button>
      </div>
    </header>
  );
}
