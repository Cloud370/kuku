import { FileCode2, GitCompareArrows } from 'lucide-react';

export type ReviewMode = 'files' | 'changes';

export function ReviewModeTabs({
  mode,
  onChange,
}: {
  mode: ReviewMode;
  onChange: (mode: ReviewMode) => void;
}) {
  return (
    <div aria-label="Review mode" className="flex h-10 items-center gap-1" role="tablist">
      <button
        aria-selected={mode === 'files'}
        className="inline-flex h-8 items-center gap-2 rounded-[var(--radius-sm)] px-3 text-sm aria-selected:bg-[var(--color-surface-raised)] aria-selected:text-[var(--color-text-primary)]"
        onClick={() => {
          onChange('files');
        }}
        role="tab"
        type="button"
      >
        <FileCode2 aria-hidden="true" size={15} />
        Files
      </button>
      <button
        aria-selected={mode === 'changes'}
        className="inline-flex h-8 items-center gap-2 rounded-[var(--radius-sm)] px-3 text-sm aria-selected:bg-[var(--color-surface-raised)] aria-selected:text-[var(--color-text-primary)]"
        onClick={() => {
          onChange('changes');
        }}
        role="tab"
        type="button"
      >
        <GitCompareArrows aria-hidden="true" size={15} />
        Changes
      </button>
    </div>
  );
}
