import { type HTMLAttributes } from "react";
import { cn } from "@/lib/cn";

export type CardProps = HTMLAttributes<HTMLDivElement>;

function Card({ className, ...props }: CardProps) {
  return (
    <div
      className={cn(
        "rounded-[var(--radius-lg)] border border-[var(--color-border)] bg-[var(--color-surface-raised)]",
        "shadow-[var(--shadow-card)]",
        className,
      )}
      {...props}
    />
  );
}

function CardHeader({ className, ...props }: CardProps) {
  return (
    <div
      className={cn("px-4 py-3 border-b border-[var(--color-border)] text-[var(--text-sm)] font-medium", className)}
      {...props}
    />
  );
}

function CardBody({ className, ...props }: CardProps) {
  return <div className={cn("p-4", className)} {...props} />;
}

function CardFooter({ className, ...props }: CardProps) {
  return (
    <div className={cn("px-4 py-3 border-t border-[var(--color-border)]", className)} {...props} />
  );
}

Card.Header = CardHeader;
Card.Body = CardBody;
Card.Footer = CardFooter;

export { Card };
