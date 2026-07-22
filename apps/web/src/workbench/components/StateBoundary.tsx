import type { ReactNode } from 'react';

import type { WorkbenchSnapshot } from '../state';
import { ConnectionBanner } from './ConnectionBanner';

interface StateBoundaryProps {
  children: ReactNode;
  onRetry: () => void;
  snapshot: WorkbenchSnapshot;
}

export function StateBoundary({ children, onRetry, snapshot }: StateBoundaryProps) {
  if (snapshot.projection === null) {
    if (snapshot.connection === 'error') {
      return (
        <section className="grid min-h-full place-items-center p-6" role="alert">
          <div className="text-center">
            <h2 className="text-sm font-semibold">Task unavailable</h2>
            <p className="mt-1 text-sm text-[var(--color-text-secondary)]">
              {snapshot.lastError ?? 'The Task could not be loaded.'}
            </p>
            <button
              className="mt-3 border border-[var(--color-border)] px-3 py-1.5 text-sm font-medium focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              onClick={onRetry}
              type="button"
            >
              Retry
            </button>
          </div>
        </section>
      );
    }
    if (snapshot.connection === 'loading' || snapshot.connection === 'reconnecting') {
      return (
        <section className="grid min-h-full place-items-center p-6" role="status">
          Loading Task
        </section>
      );
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <ConnectionBanner state={snapshot.connection} />
      {children}
    </div>
  );
}
