import type { SkillCatalogEntry } from '../../api/generated';

interface SkillPreviewProps {
  entry: SkillCatalogEntry;
}

export function SkillPreview({ entry }: SkillPreviewProps) {
  return (
    <section
      aria-label={`${entry.name} preview`}
      className="border border-[var(--color-border)] bg-[var(--color-surface-raised)] p-3 text-xs"
      role="region"
    >
      <h3 className="font-medium">{entry.name}</h3>
      <p className="mt-1 text-[var(--color-text-secondary)]">{entry.description}</p>
      <p className="mt-2 text-[var(--color-text-secondary)]">{entry.skill_id}</p>
    </section>
  );
}
