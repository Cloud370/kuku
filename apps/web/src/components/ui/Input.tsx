import { forwardRef, type InputHTMLAttributes } from "react";
import { cn } from "@/lib/cn";

const variantStyles = {
  default:
    "bg-[var(--color-surface)] border border-[var(--color-border)] focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)]",
  ghost:
    "bg-transparent border border-transparent focus:border-[var(--color-border)] focus:bg-[var(--color-surface-raised)]",
} as const;

type InputVariant = keyof typeof variantStyles;

export type InputProps = InputHTMLAttributes<HTMLInputElement> & {
  variant?: InputVariant;
};

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ variant = "default", className, ...props }, ref) => {
    return (
      <input
        ref={ref}
        data-variant={variant}
        className={cn(
          "w-full px-3 py-2 text-[var(--text-sm)] text-[var(--color-text-primary)] rounded-[var(--radius-md)]",
          "outline-none transition-colors",
          "placeholder:text-[var(--color-text-muted)]",
          "disabled:opacity-40 disabled:cursor-not-allowed",
          variantStyles[variant],
          className,
        )}
        {...props}
      />
    );
  },
);

Input.displayName = "Input";
