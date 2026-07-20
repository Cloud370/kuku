import type { WorkbenchSnapshot } from '../state';

const labels: Partial<Record<WorkbenchSnapshot['connection'], string>> = {
  error: 'Connection error',
  loading: 'Loading',
  offline: 'Offline',
  reconnecting: 'Reconnecting',
};

interface ConnectionBannerProps {
  state: WorkbenchSnapshot['connection'];
}

export function ConnectionBanner({ state }: ConnectionBannerProps) {
  const label = labels[state];
  return label === undefined ? null : (
    <div
      aria-live="polite"
      className="border-b border-[var(--color-border)] bg-[var(--color-surface-raised)] px-4 py-2 text-center text-xs font-medium"
      role="status"
    >
      {label}
    </div>
  );
}
