import { Search, X } from 'lucide-react';

export interface TaskFilterProps {
  value: string;
  loading: boolean;
  onChange: (value: string) => void;
}

export function TaskFilter({ value, loading, onChange }: TaskFilterProps) {
  return (
    <form
      role="search"
      className="relative"
      onSubmit={(event) => {
        event.preventDefault();
      }}
    >
      <Search
        aria-hidden="true"
        className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-[var(--color-text-muted)]"
      />
      <input
        type="search"
        aria-label="Search Tasks"
        autoComplete="off"
        value={value}
        onChange={(event) => {
          onChange(event.currentTarget.value);
        }}
        className="h-9 w-full rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)] py-2 pl-8 pr-9 text-[var(--text-sm)] text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-muted)] focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)]"
      />
      {value.length > 0 ? (
        <button
          type="button"
          aria-label="Clear Task search"
          title="Clear Task search"
          className="absolute right-1 top-1/2 flex size-7 -translate-y-1/2 items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          onClick={() => {
            onChange('');
          }}
        >
          <X aria-hidden="true" className="size-4" />
        </button>
      ) : null}
      <span className="sr-only" role="status" aria-live="polite">
        {loading ? 'Searching Tasks' : ''}
      </span>
    </form>
  );
}
