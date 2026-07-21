import { Monitor, Moon, Sun } from 'lucide-react';
import { useEffect, useState, type ComponentType } from 'react';

import {
  readThemePreference,
  resolveTheme,
  writeThemePreference,
  type ResolvedTheme,
  type ThemePreference,
} from './themePreference';

interface ThemeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

interface ThemeControlProps {
  storage?: ThemeStorage;
}

const options: ReadonlyArray<{
  Icon: ComponentType<{ 'aria-hidden': true; size: number }>;
  label: string;
  value: ThemePreference;
}> = [
  { Icon: Monitor, label: 'System theme', value: 'system' },
  { Icon: Sun, label: 'Light theme', value: 'light' },
  { Icon: Moon, label: 'Dark theme', value: 'dark' },
];

export function ThemeControl({ storage = localStorage }: ThemeControlProps) {
  const [preference, setPreference] = useState<ThemePreference>(() => readThemePreference(storage));

  useEffect(() => {
    if (preference !== 'system') {
      document.documentElement.dataset.theme = preference;
      return;
    }
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const apply = (matches: boolean) => {
      const system: ResolvedTheme = matches ? 'dark' : 'light';
      document.documentElement.dataset.theme = resolveTheme({ preference, system });
    };
    apply(media.matches);
    const update = (event: MediaQueryListEvent) => {
      apply(event.matches);
    };
    media.addEventListener('change', update);
    return () => {
      media.removeEventListener('change', update);
    };
  }, [preference]);

  return (
    <div
      aria-label="Theme"
      className="inline-flex border border-[var(--color-border)]"
      role="group"
    >
      {options.map(({ Icon, label, value }) => (
        <button
          aria-label={label}
          aria-pressed={preference === value}
          className="inline-flex size-9 items-center justify-center border-r border-[var(--color-border)] last:border-r-0 aria-pressed:bg-[var(--color-accent-muted)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          key={value}
          onClick={() => {
            setPreference(value);
            writeThemePreference(storage, value);
          }}
          title={label}
          type="button"
        >
          <Icon aria-hidden={true} size={16} />
        </button>
      ))}
    </div>
  );
}
