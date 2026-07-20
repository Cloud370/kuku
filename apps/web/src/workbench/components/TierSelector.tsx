import { Check, ChevronDown } from 'lucide-react';
import { useState } from 'react';

import type { TierCatalogEntry } from '../../api/generated';

interface TierSelectorProps {
  onChange: (tierId: string) => void;
  selectedTierId: string | null;
  tiers: TierCatalogEntry[];
}

export function TierSelector({ onChange, selectedTierId, tiers }: TierSelectorProps) {
  const [open, setOpen] = useState(false);
  const selected = tiers.find(({ tier }) => tier.tier_id === selectedTierId) ?? tiers[0];
  const label = selected?.tier.label ?? 'Choose Tier';

  return (
    <div className="relative">
      <button
        aria-label="Choose Tier"
        aria-expanded={open}
        aria-haspopup="listbox"
        className="inline-flex min-w-0 items-center gap-1 rounded-[var(--radius-sm)] border border-[var(--color-border)] px-2 py-1 text-xs font-medium hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
        onClick={() => {
          setOpen((current) => !current);
        }}
        type="button"
      >
        <span className="truncate">{label}</span>
        <ChevronDown aria-hidden="true" size={14} />
      </button>
      {open ? (
        <div
          aria-label="Tiers"
          className="absolute bottom-full left-0 z-20 mb-2 min-w-64 border border-[var(--color-border)] bg-[var(--color-surface-raised)] p-1 shadow-lg"
          role="listbox"
        >
          {tiers.map(({ tier }) => (
            <button
              aria-label={tier.label}
              aria-selected={tier.tier_id === selectedTierId}
              className="flex w-full items-start gap-2 px-2 py-2 text-left text-xs hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              key={tier.tier_id}
              onClick={() => {
                onChange(tier.tier_id);
                setOpen(false);
              }}
              role="option"
              type="button"
            >
              <Check
                aria-hidden="true"
                className={tier.tier_id === selectedTierId ? 'visible mt-0.5' : 'invisible mt-0.5'}
                size={13}
              />
              <span className="min-w-0">
                <span className="block font-medium">{tier.label}</span>
                <span className="block text-[var(--color-text-secondary)]">{tier.purpose}</span>
              </span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
