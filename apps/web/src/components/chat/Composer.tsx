import { useState, useCallback, type KeyboardEvent } from "react";
import { cn } from "@/lib/cn";

const models = ["claude-opus-4-7", "claude-sonnet-4-6", "claude-haiku-4-5"];

export type ComposerProps = {
  onSubmit?: (text: string, model: string) => void;
  disabled?: boolean;
  error?: string | null;
  onDismissError?: () => void;
};

export function Composer({ onSubmit, disabled, error, onDismissError }: ComposerProps) {
  const [value, setValue] = useState("");
  const [model, setModel] = useState(models[0] ?? "claude-opus-4-7");

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
        <div className="flex items-center justify-between mb-2 px-3 py-1.5 rounded-[var(--radius-md)] bg-[var(--color-error)] border border-[var(--color-error-border)] text-[var(--text-xs)] text-red-400">
          <span>{error}</span>
          <button
            onClick={onDismissError}
            className="ml-2 text-red-400 hover:text-red-300 cursor-pointer shrink-0"
            aria-label="Dismiss error"
          >
            &#x2715;
          </button>
        </div>
      )}
      <div className="flex items-start gap-2">
        <textarea
          value={value}
          onChange={(e) => setValue(e.target.value)}
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
        <div className="flex flex-col gap-2 shrink-0">
          <select
            value={model}
            onChange={(e) => setModel(e.target.value)}
            disabled={disabled}
            className={cn(
              "text-[var(--text-xs)] text-[var(--color-text-secondary)] bg-[var(--color-surface-raised)]",
              "border border-[var(--color-border)] rounded-[var(--radius-sm)] px-2 py-1.5",
              "outline-none cursor-pointer",
              "disabled:opacity-40 disabled:cursor-not-allowed",
            )}
          >
            {models.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
          <button
            onClick={handleSend}
            disabled={disabled || value.trim() === ""}
            className={cn(
              "px-4 py-1.5 text-[var(--text-sm)] font-medium rounded-[var(--radius-md)] transition-colors cursor-pointer",
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
