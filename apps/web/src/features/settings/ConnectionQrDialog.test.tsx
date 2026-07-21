import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import platformStatusJson from '../../api/generated/fixtures/platform_status.json';
import type { PlatformStatus } from '../../api/generated';
import { ConnectionQrDialog } from './ConnectionQrDialog';

const info = (structuredClone(platformStatusJson) as PlatformStatus).connection;

afterEach(cleanup);

describe('ConnectionQrDialog', () => {
  it('encodes the fragment credential without rendering the secret as text', async () => {
    const encode = vi.fn().mockResolvedValue('data:image/png;base64,fixture');
    render(
      <ConnectionQrDialog
        credentials={{ current: () => 'fixture-token' }}
        encoder={{ encode }}
        info={info}
        open
      />,
    );

    await waitFor(() => {
      expect(encode).toHaveBeenCalledWith('http://phone-host:17777/#credential=fixture-token');
    });
    expect(screen.queryByText(/fixture-token/)).toBeNull();
    expect(await screen.findByRole('img', { name: 'Connection QR code' })).toBeVisible();
  });

  it('does not encode a QR without a browser credential', () => {
    const encode = vi.fn();
    render(
      <ConnectionQrDialog
        credentials={{ current: () => null }}
        encoder={{ encode }}
        info={info}
        open
      />,
    );

    expect(screen.getByText('Connection QR unavailable on this browser')).toBeVisible();
    expect(encode).not.toHaveBeenCalled();
  });

  it('reads the credential only while open and discards the rendered QR on close', async () => {
    const current = vi.fn().mockReturnValue('fixture-token');
    const view = render(
      <ConnectionQrDialog
        credentials={{ current }}
        encoder={{ encode: vi.fn().mockResolvedValue('data:image/png;base64,fixture') }}
        info={info}
        open
      />,
    );
    expect(await screen.findByRole('img', { name: 'Connection QR code' })).toBeVisible();

    view.rerender(
      <ConnectionQrDialog
        credentials={{ current }}
        encoder={{ encode: vi.fn() }}
        info={info}
        open={false}
      />,
    );
    expect(screen.queryByRole('img', { name: 'Connection QR code' })).toBeNull();
    expect(current).toHaveBeenCalledTimes(1);
  });
});
