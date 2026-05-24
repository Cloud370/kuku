import { useState, useCallback, type KeyboardEvent } from "react";
import { cn } from "@/lib/cn";

const models = [
  { id: "claude-opus-4-7", label: "Opus" },
  { id: "claude-sonnet-4-6", label: "Sonnet" },
  { id: "claude-haiku-4-5", label: "Haiku" },
];

export type ComposerProps = {
  onSubmit?: (text: string, model: string) => void;
  disabled?: boolean;
  error?: string | null;
  onDismissError?: () => void;
};

export function Composer({ onSubmit, disabled, error, onDismissError }: ComposerProps) {
  const [value, setValue] = useState("");
  const [model, setModel] = useState(models[0]?.id ?? "claude-opus-4-7");

  const handleSend = useCallback(() => {
    const trimmed = value.trim();
    if (trimmed === "" || disabled) return;
    onSubmit?.(trimmed, model);
    setValue("");
  }, [value, model, disabled, onSubmit]);

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <div className="shrink-0 border-t border-[var(--color-border)] bg-[var(--color-surface)] px-4 py-3">
      {error && (
        <div className="flex items-center justify-between mb-3 px-3 py-1.5 rounded-[var(--radius-md)] bg-[var(--color-error)] border border-[var(--color-error-border)] text-[var(--text-xs)] text-red-400">
          <span>{error}</span>
          <button
            onClick={() => { onDismissError?.(); }}
            className="ml-2 text-red-400 hover:text-red-300 cursor-pointer shrink-0"
            aria-label="Dismiss error"
          >
            &#x2715;
          </button>
        </div>
      )}
      <div className="flex flex-col gap-2">
        <div className="flex gap-1">
          {models.map((m) => (
            <button
              key={m.id}
              onClick={() => { setModel(m.id); }}
              disabled={disabled}
              className={cn(
                "px-2.5 py-0.5 text-[var(--text-xs)] rounded-[var(--radius-full)] transition-colors cursor-pointer border",
                m.id === model
                  ? "bg-[var(--color-accent)] text-white border-[var(--color-accent)]"
                  : "text-[var(--color-text-muted)] border-[var(--color-border)] hover:text-[var(--color-text-secondary)] hover:border-[var(--color-border-strong)]",
                "disabled:opacity-40 disabled:cursor-not-allowed",
              )}
            >
              {m.label}
            </button>
          ))}
        </div>
        <div className="flex items-start gap-2">
          <textarea
            value={value}
            onChange={(e) => { setValue(e.target.value); }}
            onKeyDown={handleKeyDown}
            disabled={disabled}
            rows={2}
            placeholder="Type a message... (Enter to send, Shift+Enter for new line)"
            className={cn(
              "flex-1 resize-none rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface-raised)]",
              "px-3 py-2 text-[var(--text-sm)] text-[var(--color-text-primary)]",
              "placeholder:text-[var(--color-text-muted)]",
              "outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)]",
              "disabled:opacity-40 disabled:cursor-not-allowed",
              "transition-colors",
            )}
          />
          <button
            onClick={() => { handleSend(); }}
            disabled={disabled || value.trim() === ""}
            className={cn(
              "px-4 py-2 text-[var(--text-sm)] font-medium rounded-[var(--radius-md)] transition-colors cursor-pointer shrink-0",
              "bg-[var(--color-accent)] text-white",
              "hover:opacity-90 active:opacity-80",
              "disabled:opacity-40 disabled:cursor-not-allowed",
            )}
          >
            Send
          </button>
        </div>
      </div>
    </div>
  );
}
