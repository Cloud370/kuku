import { useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import type { SendTiming } from "@/api/sessions";
import { cn } from "@/lib/cn";
import { Button } from "@/components/ui/Button";

export type ComposerProps = {
  agent?: string;
  busy?: boolean;
  jobNeedingReview?: string;
  value?: string;
  onChange?: (value: string) => void;
  onSubmit?: (text: string, timing: SendTiming) => void;
  disabled?: boolean;
  error?: string | null;
  onDismissError?: () => void;
  modelControl?: ReactNode;
};

export function Composer({
  agent,
  busy,
  jobNeedingReview,
  value,
  onChange,
  onSubmit,
  disabled,
  error,
  onDismissError,
  modelControl,
}: ComposerProps) {
  const [localValue, setLocalValue] = useState("");
  const text = value ?? localValue;
  const empty = !text.trim();

  function handleSend(timing: SendTiming) {
    if (empty || disabled) return;
    onSubmit?.(text.replace(/\r\n?/g, "\n"), timing);
    if (value === undefined) setLocalValue("");
  }

  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      handleSend(event.altKey ? "next_turn" : "steer");
    }
  }

  return (
    <div className="shrink-0 border-t border-[var(--color-border)] bg-[var(--color-surface)] px-4 py-3">
      {error && (
        <div
          role="alert"
          className="flex items-center justify-between mb-3 px-3 py-1.5 rounded-[var(--radius-md)] bg-[var(--color-error)] border border-[var(--color-error-border)] text-[var(--text-xs)] text-red-400"
        >
          <span>{error}</span>
          {onDismissError && (
            <button
              onClick={onDismissError}
              className="ml-2 text-red-400 hover:text-red-300 cursor-pointer shrink-0"
              aria-label="Dismiss error"
            >
              &#x2715;
            </button>
          )}
        </div>
      )}
      {jobNeedingReview && (
        <p role="status" className="mb-2 text-xs text-[var(--color-text-muted)]">
          Messages will be queued until you review {jobNeedingReview} in the terminal.
        </p>
      )}
      <div className="flex flex-col gap-2">
        {(agent || modelControl) && (
          <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-[var(--color-text-muted)]">
            {agent && (
              <span>
                Message to <strong>{agent}</strong>
              </span>
            )}
            {modelControl}
          </div>
        )}
        <div className="flex flex-wrap items-start gap-2">
          <textarea
            aria-label="Message"
            value={text}
            onChange={(event) => {
              (onChange ?? setLocalValue)(event.target.value);
            }}
            onKeyDown={handleKeyDown}
            rows={2}
            placeholder="Enter to send · Shift+Enter for a new line"
            className={cn(
              "flex-1 min-w-36 resize-y rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface-raised)]",
              "px-3 py-2 text-[var(--text-sm)] text-[var(--color-text-primary)]",
              "placeholder:text-[var(--color-text-muted)]",
              "outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)]",
            )}
          />
          <div className="flex flex-col gap-1 shrink-0">
            <Button
              disabled={disabled || empty}
              onClick={() => {
                handleSend("steer");
              }}
            >
              {jobNeedingReview ? "Queue message" : busy ? "Steer" : "Send"}
            </Button>
            {busy && (
              <Button
                variant="secondary"
                size="sm"
                disabled={disabled || empty}
                onClick={() => {
                  handleSend("next_turn");
                }}
              >
                Queue for next turn
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
