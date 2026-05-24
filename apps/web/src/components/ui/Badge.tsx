import { type HTMLAttributes } from "react";
import { cn } from "@/lib/cn";

const variantStyles = {
  default: "bg-[var(--color-surface-hover)] text-[var(--color-text-secondary)]",
  warning: "bg-[var(--color-warning)] text-yellow-300 border border-[var(--color-warning-border)]",
  error: "bg-[var(--color-error)] text-red-400 border border-[var(--color-error-border)]",
  success: "bg-[var(--color-success)] text-green-400",
} as const;

type BadgeVariant = keyof typeof variantStyles;

export type BadgeProps = HTMLAttributes<HTMLSpanElement> & {
  variant?: BadgeVariant;
};

export function Badge({ variant = "default", className, ...props }: BadgeProps) {
  return (
    <span
      data-variant={variant}
      className={cn(
        "inline-flex items-center px-2 py-0.5 text-[var(--text-xs)] font-medium rounded-[var(--radius-full)]",
        variantStyles[variant],
        className,
      )}
      {...props}
    />
  );
}
