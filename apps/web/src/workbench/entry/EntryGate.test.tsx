import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import platformStatusJson from '../../api/generated/fixtures/platform_status.json';
import type {
  ApiError,
  InitPhase,
  InitStatus,
  PlatformStatus,
  TestProviderResult,
} from '../../api/generated';
import { WebApiError, webApi } from '../../api/client';

import { EntryGate } from './EntryGate';

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return {
    ...actual,
    webApi: {
      ...actual.webApi,
      credentials: {
        set: vi.fn(),
        clear: vi.fn(),
        current: vi.fn(),
      },
      platform: { status: vi.fn() },
      init: {
        status: vi.fn(),
        providers: vi.fn(),
        defaultTier: vi.fn(),
        workspace: vi.fn(),
        test: vi.fn(),
        complete: vi.fn(),
      },
    },
  };
});

const statusMock = vi.mocked(webApi.platform.status);
const initStatusMock = vi.mocked(webApi.init.status);

function apiError(code: ApiError['code']): WebApiError {
  return new WebApiError(401, {
    api_version: 1,
    code,
    details: null,
    message: code,
    trace_id: 'trace_entry_fixture',
  });
}

function initStatus(phase: InitPhase): InitStatus {
  const order: InitPhase[] = [
    'required',
    'providers_ready',
    'default_tier_ready',
    'workspace_ready',
    'probe_passed',
    'complete',
  ];
  const reached = (target: InitPhase) => order.indexOf(phase) >= order.indexOf(target);
  return {
    api_version: 1,
    complete: phase === 'complete',
    default_tier_configured: reached('default_tier_ready'),
    phase,
    provider_test_passed: reached('probe_passed'),
    providers_configured: reached('providers_ready'),
    server_revision: `revision-${phase}`,
    workspace_registered: reached('workspace_ready'),
  };
}

function platformStatus(phase: InitPhase = 'complete'): PlatformStatus {
  return {
    ...(structuredClone(platformStatusJson) as PlatformStatus),
    init: initStatus(phase),
    ready: phase === 'complete',
  };
}

function renderEntry(workbench: ReactNode = <p>Workbench</p>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <EntryGate renderWorkbench={() => workbench} />
    </QueryClientProvider>,
  );
}

async function fillProviderForm(user: ReturnType<typeof userEvent.setup>) {
  await user.type(await screen.findByLabelText('Provider ID'), 'provider-local');
  await user.type(screen.getByLabelText('API format'), 'openai');
  await user.type(screen.getByLabelText('Base URL'), 'http://127.0.0.1:9000');
  await user.type(screen.getByLabelText('Provider credential'), 'provider-secret');
  await user.type(screen.getByLabelText('Tier ID'), 'tier:local:balanced');
  await user.type(screen.getByLabelText('Model'), 'fixture-model');
}

describe('EntryGate', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    history.replaceState(null, '', '/');
  });

  afterEach(() => {
    cleanup();
    history.replaceState(null, '', '/');
  });

  it('shows a status while the server entry state is loading', () => {
    statusMock.mockReturnValue(new Promise(() => undefined));

    renderEntry();

    expect(screen.getByRole('status')).toHaveTextContent('Connecting to kuku');
    expect(screen.queryByText('Workbench')).toBeNull();
  });

  it('shows Auth and does not expose Workbench for an unauthenticated response', async () => {
    statusMock.mockRejectedValue(apiError('auth_required'));

    renderEntry();

    expect(await screen.findByRole('heading', { name: 'Connect to kuku' })).toBeVisible();
    expect(screen.queryByText('Workbench')).toBeNull();
    expect(screen.queryByRole('button', { name: /skip/i })).toBeNull();
  });

  it('stores one fragment credential, removes it, and verifies it through status', async () => {
    statusMock.mockResolvedValue(platformStatus());
    history.replaceState(null, '', '/#credential=fixture-token');

    renderEntry();

    expect(await screen.findByText('Workbench')).toBeVisible();
    expect(vi.mocked(webApi.credentials).set.mock.calls).toEqual([['fixture-token']]);
    expect(statusMock).toHaveBeenCalledTimes(1);
    expect(location.hash).toBe('');
  });

  it('clears a rejected credential when authenticated status returns auth_required', async () => {
    const user = userEvent.setup();
    statusMock.mockRejectedValue(apiError('auth_required'));
    renderEntry();

    await user.type(await screen.findByLabelText('Credential'), 'wrong-token');
    await user.click(screen.getByRole('button', { name: 'Connect' }));

    expect(vi.mocked(webApi.credentials).set.mock.calls).toContainEqual(['wrong-token']);
    expect(vi.mocked(webApi.credentials).clear.mock.calls).toHaveLength(1);
    expect(screen.getByRole('alert')).toHaveTextContent('Credential was rejected');
  });

  it('requires Init after authentication and resumes from the canonical Init status', async () => {
    statusMock.mockResolvedValue(platformStatus('default_tier_ready'));
    initStatusMock.mockResolvedValue(initStatus('default_tier_ready'));

    renderEntry();

    expect(await screen.findByRole('heading', { name: 'Initialize kuku' })).toBeVisible();
    expect(await screen.findByRole('button', { name: 'Register workspace' })).toBeVisible();
    expect(screen.queryByText('Workbench')).toBeNull();
  });

  it('dispatches the providers phase through only its exact typed operation', async () => {
    const user = userEvent.setup();
    statusMock.mockResolvedValue(platformStatus('required'));
    initStatusMock
      .mockResolvedValueOnce(initStatus('required'))
      .mockResolvedValueOnce(initStatus('providers_ready'));
    vi.mocked(webApi.init.providers).mockResolvedValue(initStatus('providers_ready'));
    renderEntry();

    await fillProviderForm(user);
    await user.click(screen.getByRole('button', { name: 'Save providers' }));

    await waitFor(() => {
      expect(initStatusMock).toHaveBeenCalledTimes(2);
    });
    expect(webApi.init.providers).toHaveBeenCalledWith({
      expected_revision: 'revision-required',
      providers: [
        {
          base_url: 'http://127.0.0.1:9000',
          credential: { source: 'direct_value', value: 'provider-secret' },
          format: 'openai',
          provider_id: 'provider-local',
        },
      ],
      tiers: [
        {
          model: 'fixture-model',
          provider_id: 'provider-local',
          purpose: 'General use',
          think: null,
          tier_id: 'tier:local:balanced',
        },
      ],
    });
    expect(webApi.init.defaultTier).not.toHaveBeenCalled();
  });

  it.each([
    {
      phase: 'providers_ready' as const,
      button: 'Save default Tier',
      field: 'Default Tier ID',
      value: 'tier:local:balanced',
      operation: 'defaultTier' as const,
      body: {
        expected_revision: 'revision-providers_ready',
        tier_id: 'tier:local:balanced',
      },
      next: 'default_tier_ready' as const,
    },
    {
      phase: 'default_tier_ready' as const,
      button: 'Register workspace',
      field: 'Workspace label',
      value: 'kuku',
      operation: 'workspace' as const,
      body: {
        workspace: {
          expected_revision: 'revision-default_tier_ready',
          label: 'kuku',
          relative_path: '.',
          root_id: 'root:default',
        },
      },
      next: 'workspace_ready' as const,
    },
    {
      phase: 'workspace_ready' as const,
      button: 'Test provider',
      field: 'Test Tier ID',
      value: 'tier:local:balanced',
      operation: 'test' as const,
      body: {
        expected_revision: 'revision-workspace_ready',
        tier_id: 'tier:local:balanced',
      },
      next: 'probe_passed' as const,
    },
  ])('calls only the $operation operation for $phase', async (entry) => {
    const user = userEvent.setup();
    statusMock.mockResolvedValue(platformStatus(entry.phase));
    initStatusMock
      .mockResolvedValueOnce(initStatus(entry.phase))
      .mockResolvedValueOnce(initStatus(entry.next));
    if (entry.operation === 'test') {
      const result: TestProviderResult = {
        api_version: 1,
        message: null,
        model: 'fixture-model',
        provider: 'provider-local',
        reachable: true,
      };
      vi.mocked(webApi.init.test).mockResolvedValue(result);
    } else {
      vi.mocked(webApi.init[entry.operation]).mockResolvedValue(initStatus(entry.next));
    }
    renderEntry();

    await user.clear(await screen.findByLabelText(entry.field));
    await user.type(screen.getByLabelText(entry.field), entry.value);
    await user.click(screen.getByRole('button', { name: entry.button }));

    await waitFor(() => {
      expect(initStatusMock).toHaveBeenCalledTimes(2);
    });
    expect(webApi.init[entry.operation]).toHaveBeenCalledWith(entry.body);
  });

  it('completes Init through the typed operation and refetches platform status', async () => {
    const user = userEvent.setup();
    statusMock
      .mockResolvedValueOnce(platformStatus('probe_passed'))
      .mockResolvedValueOnce(platformStatus('complete'));
    initStatusMock
      .mockResolvedValueOnce(initStatus('probe_passed'))
      .mockResolvedValueOnce(initStatus('complete'));
    vi.mocked(webApi.init.complete).mockResolvedValue(initStatus('complete'));
    renderEntry();

    await user.click(await screen.findByRole('button', { name: 'Complete setup' }));

    expect(await screen.findByText('Workbench')).toBeVisible();
    expect(webApi.init.complete).toHaveBeenCalledWith({
      expected_revision: 'revision-probe_passed',
    });
    expect(statusMock).toHaveBeenCalledTimes(2);
  });

  it('offers a retry without exposing Workbench for non-auth status failures', async () => {
    const user = userEvent.setup();
    statusMock.mockRejectedValueOnce(apiError('internal')).mockResolvedValueOnce(platformStatus());
    renderEntry();

    expect(await screen.findByRole('alert')).toHaveTextContent('Server status is unavailable');
    expect(screen.queryByText('Workbench')).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText('Workbench')).toBeVisible();
  });
});
