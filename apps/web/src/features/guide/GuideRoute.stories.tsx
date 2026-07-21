import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { Meta, StoryObj } from '@storybook/react';

import platformCatalogJson from '../../api/generated/fixtures/platform_catalog.json';
import type { PlatformCatalog } from '../../api/generated';
import { GuideRoute } from './GuideRoute';

const meta = {
  component: GuideRoute,
  decorators: [
    (Story) => (
      <QueryClientProvider client={new QueryClient()}>
        <Story />
      </QueryClientProvider>
    ),
  ],
  title: 'Experience/Guide',
} satisfies Meta<typeof GuideRoute>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Actions: Story = {
  args: {
    api: {
      catalog: {
        platform: () => Promise.resolve(structuredClone(platformCatalogJson) as PlatformCatalog),
      },
    },
    onPrefillFirstTask: () => undefined,
  },
};
