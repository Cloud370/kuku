import { useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

type ToolStatus = "running" | "completed" | "error";
type ToolKind = "tool" | "agent";

export type ToolCardProps = {
  icon?: string;
  name: string;
  summary: string;
  status: ToolStatus;
  kind?: ToolKind;
  childSessionId?: string;
  children?: ReactNode;
};

const statusIcon: Record<ToolStatus, string> = {
  running: "⏳",
  completed: "✅",
  error: "❌",
};

export function ToolCard({
  icon = "🔧",
  name,
  summary,
  status,
  kind,
  childSessionId,
  children,
}: ToolCardProps) {
  const [open, setOpen] = useState(false);

  return (
    <div
      className={cn(
        "mb-2 rounded-[var(--radius-md)] border bg-[var(--color-surface)] overflow-hidden",
        status === "error"
          ? "border-l-2 border-l-red-400 border-[var(--color-error-border)]"
          : "border-[var(--color-border)]",
        status === "running" && "animate-shimmer",
      )}
    >
      <button
        onClick={() => { setOpen(!open); }}
        className="w-full flex items-center gap-2 px-3 py-2 text-left transition-colors cursor-pointer hover:bg-[var(--color-surface-hover)]"
      >
        <span className="text-[var(--text-sm)] shrink-0">{icon}</span>
        <span className="text-[var(--text-sm)] font-medium text-[var(--color-text-primary)] truncate">
          {name}
        </span>
        <span className="text-[var(--text-xs)] text-[var(--color-text-muted)] truncate flex-1">
          {summary}
        </span>
        <span className="text-[var(--text-xs)] shrink-0">{statusIcon[status]}</span>
        {(kind === "agent" || children) && (
          <span
            className={cn(
              "text-[var(--text-xs)] transition-transform shrink-0",
              open && "rotate-180",
            )}
          >
            &#x25BC;
          </span>
        )}
      </button>
      {open && (
        <div className="border-t border-[var(--color-border)] px-3 py-2">
          {children && <div className="text-[var(--text-sm)]">{children}</div>}
          {kind === "agent" && childSessionId && status === "completed" && (
            <button
              className="mt-2 text-[var(--text-xs)] text-[var(--color-accent)] hover:underline cursor-pointer"
              onClick={(e) => { e.stopPropagation(); }}
            >
              View sub-agent
            </button>
          )}
          {kind === "agent" && !children && (
            <p className="text-[var(--text-xs)] text-[var(--color-text-muted)] font-mono">
              Agent tool chain — sub-agent events will render here
            </p>
          )}
        </div>
      )}
    </div>
  );
}
