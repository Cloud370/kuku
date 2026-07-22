import { ShieldQuestion } from 'lucide-react';
import { useState } from 'react';

import type { InteractionProjection } from '../../api/generated';

interface InteractionCardProps {
  interaction: InteractionProjection;
  taskId: string;
  onRespond: (taskId: string, interactionId: string, choiceId: string) => Promise<void> | void;
}

export function InteractionCard({ interaction, taskId, onRespond }: InteractionCardProps) {
  const [confirmedChoiceId, setConfirmedChoiceId] = useState<string | null>(null);
  const [responding, setResponding] = useState(false);
  const status = confirmedChoiceId === null ? interaction.status : 'resolved';

  const respond = async (choiceId: string) => {
    setResponding(true);
    try {
      await onRespond(taskId, interaction.interaction_id, choiceId);
      setConfirmedChoiceId(choiceId);
    } finally {
      setResponding(false);
    }
  };

  return (
    <section
      aria-label="Permission request"
      className="border border-[var(--color-border)] bg-[var(--color-surface-raised)] p-4"
      role="group"
    >
      <div className="flex items-start gap-2">
        <ShieldQuestion aria-hidden="true" className="mt-0.5 shrink-0" size={18} />
        <p className="min-w-0 flex-1 text-sm font-medium">{interaction.prompt}</p>
      </div>
      {status === 'pending' ? (
        <div className="mt-3 flex flex-wrap gap-2">
          {interaction.choices.map((choice) => (
            <button
              className="rounded-[var(--radius-sm)] border border-[var(--color-border)] px-3 py-1.5 text-sm font-medium hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              key={choice.choice_id}
              onClick={() => {
                void respond(choice.choice_id).catch(() => undefined);
              }}
              disabled={responding}
              type="button"
            >
              {choice.label}
            </button>
          ))}
        </div>
      ) : (
        <p className="mt-3 text-xs text-[var(--color-text-secondary)]">
          {status === 'resolved' ? 'Resolved' : 'Cancelled'}
        </p>
      )}
    </section>
  );
}
