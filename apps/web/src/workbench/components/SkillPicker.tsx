import { Search } from 'lucide-react';
import { useMemo, useState } from 'react';

import type { SkillCatalogEntry } from '../../api/generated';

interface SkillPickerProps {
  entries: SkillCatalogEntry[];
  onSearch?: (query: string) => void;
  onStage: (entry: SkillCatalogEntry) => void;
}

export function SkillPicker({ entries, onSearch, onStage }: SkillPickerProps) {
  const [query, setQuery] = useState('');
  const filtered = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (normalized.length === 0) return entries;
    return entries.filter(
      (entry) =>
        entry.name.toLowerCase().includes(normalized) ||
        entry.description.toLowerCase().includes(normalized) ||
        entry.skill_id.toLowerCase().includes(normalized),
    );
  }, [entries, query]);

  return (
    <div className="mt-2 border border-[var(--color-border)] bg-[var(--color-surface-raised)] p-2">
      <label className="sr-only" htmlFor="skill-search">
        Search Skills
      </label>
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] pb-2">
        <Search aria-hidden="true" size={14} />
        <input
          aria-label="Search Skills"
          className="min-w-0 flex-1 bg-transparent text-xs outline-none"
          id="skill-search"
          onChange={(event) => {
            setQuery(event.target.value);
            onSearch?.(event.target.value);
          }}
          placeholder="Search Skills"
          type="search"
          value={query}
        />
      </div>
      <div aria-label="Available Skills" className="mt-1 max-h-44 overflow-y-auto" role="listbox">
        {filtered.length === 0 ? (
          <p className="px-2 py-3 text-xs text-[var(--color-text-secondary)]">No Skills found.</p>
        ) : (
          filtered.map((entry) => (
            <button
              aria-label={entry.name}
              className="block w-full px-2 py-2 text-left text-xs hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              key={entry.skill_id}
              onClick={() => {
                onStage(entry);
              }}
              role="option"
              type="button"
            >
              <span className="block font-medium">{entry.name}</span>
              <span className="block truncate text-[var(--color-text-secondary)]">
                {entry.description}
              </span>
            </button>
          ))
        )}
      </div>
    </div>
  );
}
