import { useQuery } from '@tanstack/react-query';
import { AlertTriangle, LoaderCircle } from 'lucide-react';
import { useState, type ReactNode } from 'react';

import { WebApiError, webApi } from '../../api/client';
import type { PlatformStatus } from '../../api/generated';

import { AuthScreen } from './AuthScreen';
import { InitScreen, type InitOperations } from './InitScreen';

interface EntryGateProps {
  renderWorkbench: (status: PlatformStatus) => ReactNode;
}

function importFragmentCredential(): void {
  const fragment = new URLSearchParams(location.hash.slice(1));
  const credential = fragment.get('credential');
  if (credential === null || credential.length === 0) return;
  webApi.credentials.set(credential);
  history.replaceState(history.state, '', `${location.pathname}${location.search}`);
}

function isAuthRequired(error: unknown): error is WebApiError {
  return error instanceof WebApiError && error.code === 'auth_required';
}

export function EntryGate({ renderWorkbench }: EntryGateProps) {
  useState(importFragmentCredential);
  const status = useQuery({
    queryKey: ['platform-status'],
    queryFn: webApi.platform.status,
    retry: false,
  });

  if (status.isPending) {
    return (
      <main className="flex min-h-dvh items-center justify-center" role="status">
        <LoaderCircle aria-hidden="true" className="mr-2 animate-spin" size={18} />
        Connecting to kuku
      </main>
    );
  }

  if (status.isError && isAuthRequired(status.error)) {
    return (
      <AuthScreen
        onSubmit={async (credential) => {
          webApi.credentials.set(credential);
          const result = await status.refetch();
          if (result.error !== null) {
            webApi.credentials.clear();
            throw result.error;
          }
        }}
        serverName="kuku"
      />
    );
  }

  if (status.isError) {
    return (
      <main className="flex min-h-dvh items-center justify-center p-6">
        <section className="max-w-md text-center" role="alert">
          <AlertTriangle
            aria-hidden="true"
            className="mx-auto mb-3 text-[var(--color-error)]"
            size={24}
          />
          <p>Server status is unavailable.</p>
          <button
            className="mt-4 rounded-[var(--radius-md)] border border-[var(--color-border)] px-4 py-2 text-sm font-medium"
            onClick={() => void status.refetch()}
            type="button"
          >
            Retry
          </button>
        </section>
      </main>
    );
  }

  if (status.data.init.phase !== 'complete') {
    const operations: InitOperations = webApi.init;
    return (
      <InitScreen onComplete={async () => void (await status.refetch())} operations={operations} />
    );
  }

  return renderWorkbench(status.data);
}
