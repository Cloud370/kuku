import { ChevronDown } from 'lucide-react';
import { useId, type ReactNode } from 'react';

import type { ContextSectionKey } from './contextSections';
import styles from './ContextPanel.module.css';

export interface ContextAccordionItem {
  key: ContextSectionKey;
  label: string;
  count?: number;
  warning?: boolean;
  content: ReactNode;
}

interface ContextAccordionProps {
  items: ContextAccordionItem[];
  openSections: ContextSectionKey[];
  onOpenSectionsChange: (keys: ContextSectionKey[]) => void;
}

export function ContextAccordion({
  items,
  openSections,
  onOpenSectionsChange,
}: ContextAccordionProps) {
  const baseId = useId();
  const toggle = (key: ContextSectionKey) => {
    if (openSections.includes(key)) {
      onOpenSectionsChange(openSections.filter((value) => value !== key));
    } else {
      onOpenSectionsChange([...openSections, key]);
    }
  };

  return (
    <div aria-label="Context sections" className={styles.accordion}>
      {items.map((item) => {
        const open = openSections.includes(item.key);
        const contentId = `${baseId}-${item.key}`;
        return (
          <section className={styles.section} key={item.key}>
            <h3>
              <button
                aria-controls={contentId}
                aria-expanded={open}
                className={`${styles.sectionButton ?? ''} hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]`}
                onClick={() => {
                  toggle(item.key);
                }}
                type="button"
              >
                <span className="min-w-0 truncate text-sm font-medium">{item.label}</span>
                {item.warning ? (
                  <span className="text-xs text-yellow-300">Attention</span>
                ) : item.count === undefined ? null : (
                  <span className="text-xs tabular-nums text-[var(--color-text-muted)]">
                    {item.count}
                  </span>
                )}
                <ChevronDown
                  aria-hidden="true"
                  className={`transition-transform motion-reduce:transition-none ${open ? 'rotate-180' : ''}`}
                  size={16}
                />
              </button>
            </h3>
            {open ? (
              <div className={styles.sectionContent} id={contentId}>
                {item.content}
              </div>
            ) : null}
          </section>
        );
      })}
    </div>
  );
}
