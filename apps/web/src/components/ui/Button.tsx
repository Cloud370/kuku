import { type ButtonHTMLAttributes } from "react";
import { cn } from "@/lib/cn";

const variantStyles = {
  primary:
    "bg-[var(--color-accent)] text-white hover:opacity-90 active:opacity-80 disabled:opacity-40",
  secondary:
    "bg-[var(--color-surface-raised)] text-[var(--color-text-primary)] border border-[var(--color-border)] hover:bg-[var(--color-surface-hover)] active:opacity-80 disabled:opacity-40",
  ghost:
    "text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] active:opacity-80 disabled:opacity-40",
  danger:
    "bg-[var(--color-error)] text-red-400 border border-[var(--color-error-border)] hover:opacity-90 active:opacity-80 disabled:opacity-40",
} as const;

const sizeStyles = {
  sm: "px-3 py-1.5 text-[var(--text-xs)] rounded-[var(--radius-sm)]",
  md: "px-4 py-2 text-[var(--text-sm)] rounded-[var(--radius-md)]",
} as const;

type ButtonVariant = keyof typeof variantStyles;
type ButtonSize = keyof typeof sizeStyles;

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
};

export function Button({
  variant = "primary",
  size = "md",
  className,
  ...props
}: ButtonProps) {
  return (
    <button
      data-variant={variant}
      data-size={size}
      className={cn(
        "inline-flex items-center justify-center font-medium transition-colors cursor-pointer select-none",
        variantStyles[variant],
        sizeStyles[size],
        className,
      )}
      {...props}
    />
  );
}
