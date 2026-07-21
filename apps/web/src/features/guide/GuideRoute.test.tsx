import '@testing-library/jest-dom/vitest';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import platformCatalogJson from '../../api/generated/fixtures/platform_catalog.json';
import { GuideAction } from './guideActions';
import { GuideRoute } from './GuideRoute';

afterEach(cleanup);

describe('GuideRoute', () => {
  it('loads the shared platform catalog and prepares an editable first Task without submitting', async () => {
    const user = userEvent.setup();
    const onPrefillFirstTask = vi.fn();
    const submitRun = vi.fn();
    const api = {
      catalog: {
        platform: vi.fn().mockResolvedValue(structuredClone(platformCatalogJson)),
      },
      tasks: { submitRun },
    };
    render(
      <QueryClientProvider client={new QueryClient()}>
        <GuideRoute api={api} onPrefillFirstTask={onPrefillFirstTask} />
      </QueryClientProvider>,
    );

    expect(await screen.findByText('Balanced')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Prepare first task' }));
    expect(onPrefillFirstTask).toHaveBeenCalledWith({
      editable: true,
      message: 'Inspect this workspace and identify the most important next step.',
    });
    expect(submitRun).not.toHaveBeenCalled();
  });

  it('exposes Init and dismisses the Guide only in local presentation state', async () => {
    const user = userEvent.setup();
    const onAction = vi.fn();
    const api = {
      catalog: { platform: vi.fn().mockResolvedValue(structuredClone(platformCatalogJson)) },
    };
    render(
      <QueryClientProvider client={new QueryClient()}>
        <GuideRoute api={api} onAction={onAction} onPrefillFirstTask={vi.fn()} />
      </QueryClientProvider>,
    );

    await user.click(await screen.findByRole('button', { name: 'Open Init' }));
    expect(onAction).toHaveBeenCalledWith(GuideAction.Init, '/init');
    await user.click(screen.getByRole('button', { name: 'Dismiss Guide' }));
    expect(screen.getByText('Guide dismissed.')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Open Init' })).toBeNull();
  });
});
