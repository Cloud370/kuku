import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

export type TurnCardProps = {
  role: "user" | "agent";
  children?: ReactNode;
  className?: string;
};

export function TurnCard({ role, children, className }: TurnCardProps) {
  return (
    <div
      className={cn(
        "px-4 py-3 rounded-[var(--radius-lg)] border border-[var(--color-border)] text-[var(--text-sm)]",
        role === "user"
          ? "ml-auto max-w-[80%] bg-[var(--color-surface-hover)] text-[var(--color-text-primary)]"
          : "mr-auto max-w-[85%] bg-[var(--color-surface-raised)] text-[var(--color-text-secondary)]",
        className,
      )}
    >
      {children}
    </div>
  );
}
