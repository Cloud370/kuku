import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { Meta, StoryObj } from '@storybook/react';

import platformCatalogJson from '../../api/generated/fixtures/platform_catalog.json';
import platformStatusJson from '../../api/generated/fixtures/platform_status.json';
import settingsSnapshotJson from '../../api/generated/fixtures/settings_snapshot.json';
import type {
  PlatformCatalog,
  PlatformStatus,
  SettingsSnapshot,
  WorkspacePage,
} from '../../api/generated';
import { SettingsRoute, type SettingsRouteApi } from './SettingsRoute';

const catalog = structuredClone(platformCatalogJson) as PlatformCatalog;
const status = structuredClone(platformStatusJson) as PlatformStatus;
const settings = structuredClone(settingsSnapshotJson) as SettingsSnapshot;
const workspaces: WorkspacePage = {
  api_version: 1,
  items: [
    {
      availability: 'available',
      branch: 'main',
      is_default: true,
      label: 'kuku',
      workspace_id: 'wsp_000000000000000000000001',
    },
  ],
  server_revision: 'revision-workspaces',
};

function api(overrides: Partial<SettingsRouteApi> = {}): SettingsRouteApi {
  return {
    catalog: { platform: () => Promise.resolve(catalog) },
    credentials: { current: () => null },
    platform: { status: () => Promise.resolve(status) },
    settings: {
      get: () => Promise.resolve(settings),
      update: () => Promise.resolve(settings),
    },
    workspaces: { list: () => Promise.resolve(workspaces) },
    ...overrides,
  };
}

const meta = {
  component: SettingsRoute,
  decorators: [
    (Story) => (
      <QueryClientProvider client={new QueryClient()}>
        <Story />
      </QueryClientProvider>
    ),
  ],
  title: 'Experience/Settings',
} satisfies Meta<typeof SettingsRoute>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Ready: Story = { args: { api: api() } };
export const Loading: Story = {
  args: {
    api: api({
      settings: {
        get: () => new Promise(() => undefined),
        update: () => new Promise(() => undefined),
      },
    }),
  },
};
export const LoadError: Story = {
  args: {
    api: api({
      settings: {
        get: () => Promise.reject(new Error('Unavailable')),
        update: () => Promise.reject(new Error('Unavailable')),
      },
    }),
  },
};
