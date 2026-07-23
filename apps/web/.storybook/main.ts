import type { StorybookConfig } from '@storybook/react-vite';

const config: StorybookConfig = {
  stories: ['../src/**/*.stories.@(ts|tsx)'],
  addons: ['@storybook/addon-a11y', '@storybook/addon-vitest'],
  framework: {
    name: '@storybook/react-vite',
    options: {},
  },
  viteFinal: async (config) => ({
    ...config,
    build: {
      ...config.build,
      chunkSizeWarningLimit: 1200,
      rolldownOptions: {
        ...config.build?.rolldownOptions,
        checks: {
          ...config.build?.rolldownOptions?.checks,
          pluginTimings: false,
        },
      },
    },
  }),
};

export default config;
