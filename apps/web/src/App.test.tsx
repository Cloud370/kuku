import '@testing-library/jest-dom/vitest';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { WorkbenchRoute } from './workbench/taskSelection';
import type { LocalDraft } from './workbench/state';
import App from './App';

interface MockWorkbenchProps {
  initialDraft?: LocalDraft;
  onNavigateTask?: (taskId: string) => void;
  route?: WorkbenchRoute;
}

vi.mock('./workbench/WorkbenchEntry', () => ({
  WorkbenchEntry: ({ initialDraft, onNavigateTask, route }: MockWorkbenchProps) => (
    <section aria-label="Production Workbench">
      <p>{route?.kind === 'task' ? `task:${route.taskId}` : (route?.kind ?? 'latest')}</p>
      <p>{initialDraft?.text ?? 'No initial draft'}</p>
      <button
        onClick={() => {
          onNavigateTask?.('tsk_000000000000000000000002');
        }}
        type="button"
      >
        Select second Task
      </button>
    </section>
  ),
}));

vi.mock('./features/review/ReviewRoute', () => ({
  ReviewRoute: ({ taskId }: { taskId: string }) => (
    <main aria-label="Production Review">review:{taskId}</main>
  ),
}));

vi.mock('./features/settings/SettingsRoute', () => ({
  SettingsRoute: ({ initialDialog }: { initialDialog?: string | null }) => (
    <main aria-label="Production Settings">
      <p>settings</p>
      <p>{initialDialog ?? 'No initial Settings dialog'}</p>
    </main>
  ),
}));

vi.mock('./features/guide/GuideRoute', () => ({
  GuideRoute: ({
    onAction,
    onPrefillFirstTask,
  }: {
    onAction?: (action: string, route: string) => void;
    onPrefillFirstTask: (prefill: { editable: true; message: string }) => void;
  }) => (
    <main aria-label="Production Guide">
      <p>guide</p>
      <button
        onClick={() => {
          onPrefillFirstTask({
            editable: true,
            message: 'Inspect this workspace and identify the most important next step.',
          });
          onAction?.('first_task', '/tasks/new');
        }}
        type="button"
      >
        Prepare first task
      </button>
    </main>
  ),
}));

function LocationProbe() {
  const location = useLocation();
  return <output aria-label="Location">{location.pathname}</output>;
}

function renderAt(path: string): void {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[path]}>
        <App />
        <LocationProbe />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('App routing', () => {
  it.each([
    ['/', 'latest'],
    ['/tasks/new', 'new'],
    ['/tasks/tsk_000000000000000000000001', 'task:tsk_000000000000000000000001'],
  ])('maps %s to the production Workbench route', async (path, expected) => {
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          new Response(JSON.stringify({ ok: true, version: 'fixture', workspace: '/code/kuku' })),
        ),
    );
    renderAt(path);

    expect(await screen.findByText(expected)).toBeVisible();
  });

  it('uses the canonical Task URL when Workbench navigation selects a Task', async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(
          new Response(JSON.stringify({ ok: true, version: 'fixture', workspace: '/code/kuku' })),
        ),
    );
    renderAt('/');

    await user.click(await screen.findByRole('button', { name: 'Select second Task' }));
    expect(screen.getByRole('status', { name: 'Location' })).toHaveTextContent(
      '/tasks/tsk_000000000000000000000002',
    );
  });

  it('carries the Guide first Task prefill into the new Task Workbench', async () => {
    const user = userEvent.setup();
    renderAt('/guide');

    await user.click(screen.getByRole('button', { name: 'Prepare first task' }));

    expect(screen.getByRole('status', { name: 'Location' })).toHaveTextContent('/tasks/new');
    expect(
      screen.getByText('Inspect this workspace and identify the most important next step.'),
    ).toBeVisible();
  });

  it('maps the Connection QR deep link to the typed Settings dialog', async () => {
    renderAt('/settings?connection=qr');

    expect(await screen.findByText('connection_qr')).toBeVisible();
  });

  it.each([
    ['/auth', 'latest'],
    ['/init', 'latest'],
    [
      '/tasks/tsk_000000000000000000000001/review?workspace=wsp_000000000000000000000001',
      'review:tsk_000000000000000000000001',
    ],
    ['/settings', 'settings'],
    ['/guide', 'guide'],
  ])('maps %s to its canonical product route', async (path, expected) => {
    renderAt(path);

    expect(await screen.findByText(expected)).toBeVisible();
  });
});
