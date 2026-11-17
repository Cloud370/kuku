import { useState } from "react";
import type { SubmitEvent } from "react";
import type { QuestionRequest } from "@/api/sessions";
import { Button } from "@/components/ui/Button";

export function PendingQuestion({
  request,
  agent,
  disabled,
  onAnswer,
}: {
  request: QuestionRequest;
  agent: string;
  disabled: boolean;
  onAnswer: (text: string) => void;
}) {
  const [answer, setAnswer] = useState("");
  function reply(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (answer.trim() && !disabled) onAnswer(answer);
  }

  return (
    <form
      onSubmit={reply}
      className="shrink-0 border-y border-[var(--color-warning-border)] bg-[var(--color-warning)] px-4 py-2.5"
    >
      <p className="text-xs font-medium text-[var(--color-text-muted)]">
        {agent} is waiting for an answer · {request.id}
      </p>
      <p className="my-1 text-sm whitespace-pre-wrap">{request.text}</p>
      <div className="flex gap-2 items-start">
        <textarea
          aria-label="Answer"
          rows={2}
          value={answer}
          onChange={(event) => {
            setAnswer(event.target.value);
          }}
          className="min-w-0 w-full resize-y rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface-raised)] px-2 py-1 text-sm"
        />
        <Button type="submit" size="sm" disabled={disabled || !answer.trim()}>
          Send answer
        </Button>
      </div>
    </form>
  );
}
