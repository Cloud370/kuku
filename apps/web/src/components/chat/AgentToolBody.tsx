import { useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

export type AgentToolBodyProps = {
  children?: ReactNode;
  maxExpand?: number;
};

export function AgentToolBody({ children, maxExpand = 2 }: AgentToolBodyProps) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="ml-3 pl-3 border-l border-[var(--color-border)]">
      <div className={cn(!expanded && "max-h-48 overflow-hidden")}>
        {children}
      </div>
      {!expanded && (
        <button
          onClick={() => { setExpanded(true); }}
          className="text-[var(--text-xs)] text-[var(--color-accent)] hover:underline cursor-pointer mt-1"
        >
          Show all ({maxExpand}+ layers)
        </button>
      )}
    </div>
  );
}
