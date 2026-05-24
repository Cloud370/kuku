import { useState } from "react";
import { cn } from "@/lib/cn";

export type ThinkingBlockProps = {
  children: string;
  defaultOpen?: boolean;
};

export function ThinkingBlock({ children, defaultOpen = false }: ThinkingBlockProps) {
  const [open, setOpen] = useState(defaultOpen);

  return (
    <div className="mb-2 rounded-[var(--radius-md)] border border-[var(--color-border)] overflow-hidden">
      <button
        onClick={() => { setOpen(!open); }}
        className="w-full flex items-center gap-2 px-3 py-1.5 text-[var(--text-xs)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors cursor-pointer bg-[var(--color-surface)]"
      >
        <span className="font-mono text-[var(--text-xs)]">&#x1F9E0;</span>
        <span>Reasoning</span>
        <span className={cn("ml-auto text-[var(--text-xs)] transition-transform", open && "rotate-180")}>
          &#x25BC;
        </span>
      </button>
      {open && (
        <div className="border-t border-[var(--color-border)] px-3 py-2 bg-[var(--color-surface)]">
          <p className="font-mono text-[var(--text-xs)] text-[var(--color-text-muted)] whitespace-pre-wrap leading-relaxed">
            {children}
          </p>
        </div>
      )}
    </div>
  );
}
