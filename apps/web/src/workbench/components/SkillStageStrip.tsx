import { X } from 'lucide-react';

import type { SkillCatalogEntry } from '../../api/generated';

interface SkillStageStripProps {
  loadedSkillCount: number;
  onOpenLoadedSkills: () => void;
  onRemove: (skillId: string) => void;
  stagedEntries: SkillCatalogEntry[];
}

export function SkillStageStrip({
  loadedSkillCount,
  onOpenLoadedSkills,
  onRemove,
  stagedEntries,
}: SkillStageStripProps) {
  return (
    <div className="flex min-w-0 items-center gap-2 overflow-hidden text-xs">
      <button
        aria-label={`${String(loadedSkillCount)} Skills loaded`}
        className="shrink-0 text-[var(--color-text-secondary)] underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
        onClick={onOpenLoadedSkills}
        type="button"
      >
        {loadedSkillCount} Skills loaded
      </button>
      {stagedEntries.slice(0, 2).map((entry, index) => (
        <span
          className={`inline-flex min-w-0 max-w-40 shrink-0 items-center gap-1 border border-[var(--color-border)] px-2 py-1${index === 1 ? ' max-sm:hidden' : ''}`}
          key={entry.skill_id}
        >
          <span className="truncate">{entry.name}</span>
          <button
            aria-label={`Remove ${entry.name}`}
            className="inline-flex size-4 shrink-0 items-center justify-center hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
            onClick={() => {
              onRemove(entry.skill_id);
            }}
            title={`Remove ${entry.name}`}
            type="button"
          >
            <X aria-hidden="true" size={12} />
          </button>
        </span>
      ))}
      {stagedEntries.length > 1 ? (
        <span className="inline-flex shrink-0 border border-[var(--color-border)] px-2 py-1 text-[var(--color-text-secondary)] sm:hidden">
          +{stagedEntries.length - 1}
        </span>
      ) : null}
      {stagedEntries.length > 2 ? (
        <span className="hidden shrink-0 border border-[var(--color-border)] px-2 py-1 text-[var(--color-text-secondary)] sm:inline-flex">
          +{stagedEntries.length - 2}
        </span>
      ) : null}
    </div>
  );
}
