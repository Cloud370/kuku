import { useQuery, useQueryClient } from '@tanstack/react-query';
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

const initOperations: InitOperations = {
  ...webApi.init,
  catalog: webApi.catalog.platform,
  registrationRoots: webApi.workspaces.registrationRoots,
};

const platformStatusKey = ['platform-status'] as const;
const platformStatusTimeoutMs = 10_000;

async function loadPlatformStatus(signal: AbortSignal): Promise<PlatformStatus> {
  const controller = new AbortController();
  const abort = () => {
    controller.abort();
  };
  let timeout: number | undefined;
  const timedOut = new Promise<never>((_resolve, reject) => {
    timeout = window.setTimeout(() => {
      reject(new Error('Platform status request timed out'));
      controller.abort();
    }, platformStatusTimeoutMs);
  });
  signal.addEventListener('abort', abort, { once: true });
  try {
    return await Promise.race([webApi.platform.status(controller.signal), timedOut]);
  } finally {
    if (timeout !== undefined) window.clearTimeout(timeout);
    signal.removeEventListener('abort', abort);
  }
}

export function EntryGate({ renderWorkbench }: EntryGateProps) {
  useState(importFragmentCredential);
  const queryClient = useQueryClient();
  const [credentialEntryOpen, setCredentialEntryOpen] = useState(false);
  const status = useQuery({
    queryKey: platformStatusKey,
    queryFn: async ({ signal }) => {
      try {
        return await loadPlatformStatus(signal);
      } catch (error) {
        if (isAuthRequired(error) && webApi.credentials.current() !== null) {
          webApi.credentials.clear();
        }
        throw error;
      }
    },
    retry: false,
  });

  const openCredentialEntry = () => {
    webApi.credentials.clear();
    setCredentialEntryOpen(true);
    void queryClient.cancelQueries({ queryKey: platformStatusKey });
  };

  const authenticate = async (credential: string) => {
    webApi.credentials.set(credential);
    try {
      const result = await loadPlatformStatus(new AbortController().signal);
      queryClient.setQueryData(platformStatusKey, result);
      setCredentialEntryOpen(false);
    } catch (error) {
      webApi.credentials.clear();
      throw error;
    }
  };

  if (credentialEntryOpen || (status.isError && isAuthRequired(status.error))) {
    return <AuthScreen onSubmit={authenticate} serverName="kuku" />;
  }

  if (status.isPending) {
    return (
      <main
        className="flex min-h-dvh flex-col items-center justify-center gap-4"
        role="status"
      >
        <span className="inline-flex items-center">
          <LoaderCircle aria-hidden="true" className="mr-2 animate-spin" size={18} />
          Connecting to kuku
        </span>
        <button
          className="rounded-[var(--radius-md)] border border-[var(--color-border)] px-4 py-2 text-sm font-medium"
          onClick={openCredentialEntry}
          type="button"
        >
          Use another credential
        </button>
      </main>
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
          <div className="mt-4 flex justify-center gap-2">
            <button
              className="rounded-[var(--radius-md)] border border-[var(--color-border)] px-4 py-2 text-sm font-medium"
              onClick={() => void status.refetch()}
              type="button"
            >
              Retry
            </button>
            <button
              className="rounded-[var(--radius-md)] border border-[var(--color-border)] px-4 py-2 text-sm font-medium"
              onClick={openCredentialEntry}
              type="button"
            >
              Use another credential
            </button>
          </div>
        </section>
      </main>
    );
  }

  if (status.data.init.phase !== 'complete') {
    return (
      <InitScreen
        onComplete={async () => void (await status.refetch())}
        operations={initOperations}
      />
    );
  }

  return renderWorkbench(status.data);
}
