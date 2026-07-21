import '@testing-library/jest-dom/vitest';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactElement } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import platformCatalogJson from '../../api/generated/fixtures/platform_catalog.json';
import platformStatusJson from '../../api/generated/fixtures/platform_status.json';
import settingsSnapshotJson from '../../api/generated/fixtures/settings_snapshot.json';
import type {
  PlatformCatalog,
  PlatformStatus,
  SettingsSnapshot,
  WorkspacePage,
} from '../../api/generated';
import { SettingsRoute } from './SettingsRoute';

const catalog = structuredClone(platformCatalogJson) as PlatformCatalog;
const status = structuredClone(platformStatusJson) as PlatformStatus;
const settings = structuredClone(settingsSnapshotJson) as SettingsSnapshot;
const workspaces: WorkspacePage = {
  api_version: 1,
  items: [],
  server_revision: 'revision-workspaces',
};

function renderRoute(value: ReactElement): void {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      {value}
    </QueryClientProvider>,
  );
}

afterEach(cleanup);

describe('SettingsRoute', () => {
  it('loads platform truth and sends only the revisioned generated patch', async () => {
    const user = userEvent.setup();
    const update = vi.fn().mockResolvedValue({ ...settings, max_concurrent_runs: 8 });
    const api = {
      catalog: { platform: vi.fn().mockResolvedValue(catalog) },
      credentials: { current: () => 'fixture-token' },
      platform: { status: vi.fn().mockResolvedValue(status) },
      settings: { get: vi.fn().mockResolvedValue(settings), update },
      workspaces: { list: vi.fn().mockResolvedValue(workspaces) },
    };
    renderRoute(<SettingsRoute api={api} />);

    expect(await screen.findByText('Balanced')).toBeVisible();
    const runs = screen.getByRole('spinbutton', { name: 'Maximum concurrent runs' });
    await user.clear(runs);
    await user.type(runs, '8');
    await user.click(
      screen.getByRole('checkbox', { name: 'Discover Skills and Agents automatically' }),
    );
    await user.click(screen.getByRole('button', { name: 'Save Settings' }));

    expect(update).toHaveBeenCalledWith({
      expected_revision: settings.server_revision,
      patch: {
        default_tier: settings.default_tier,
        default_workspace_id: settings.default_workspace_id,
        max_concurrent_runs: 8,
        discovery: { auto_discover: false },
      },
    });
  });

  it('refetches after an unknown PATCH outcome and retains form values', async () => {
    const user = userEvent.setup();
    const get = vi
      .fn()
      .mockResolvedValueOnce(settings)
      .mockResolvedValueOnce({ ...settings, server_revision: 'revision-refetched' });
    const api = {
      catalog: { platform: vi.fn().mockResolvedValue(catalog) },
      credentials: { current: () => null },
      platform: { status: vi.fn().mockResolvedValue(status) },
      settings: { get, update: vi.fn().mockRejectedValue(new TypeError('network lost')) },
      workspaces: { list: vi.fn().mockResolvedValue(workspaces) },
    };
    renderRoute(<SettingsRoute api={api} />);

    const runs = await screen.findByRole('spinbutton', { name: 'Maximum concurrent runs' });
    await user.clear(runs);
    await user.type(runs, '9');
    await user.click(screen.getByRole('button', { name: 'Save Settings' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('reconciled');
    expect(runs).toHaveValue(9);
    expect(get).toHaveBeenCalledTimes(2);
  });

  it('does not submit a non-positive concurrent Run limit', async () => {
    const user = userEvent.setup();
    const api = {
      catalog: { platform: vi.fn().mockResolvedValue(catalog) },
      credentials: { current: () => null },
      platform: { status: vi.fn().mockResolvedValue(status) },
      settings: { get: vi.fn().mockResolvedValue(settings), update: vi.fn() },
      workspaces: { list: vi.fn().mockResolvedValue(workspaces) },
    };
    renderRoute(<SettingsRoute api={api} />);

    const runs = await screen.findByRole('spinbutton', { name: 'Maximum concurrent runs' });
    await user.clear(runs);
    await user.type(runs, '0');
    expect(screen.getByRole('button', { name: 'Save Settings' })).toBeDisabled();
    expect(api.settings.update).not.toHaveBeenCalled();
  });
});
