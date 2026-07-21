import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { ResponsiveFeatureSurface } from './ResponsiveFeatureSurface';

afterEach(cleanup);

describe('ResponsiveFeatureSurface', () => {
  it('keeps toolbar, scrolling content, and transition status semantically separate', () => {
    render(
      <ResponsiveFeatureSurface
        kind="settings"
        mode="full"
        status={{ kind: 'saving', message: 'Saving Settings' }}
        toolbar={<button type="button">Save</button>}
      >
        <p>Settings content</p>
      </ResponsiveFeatureSurface>,
    );

    expect(screen.getByRole('region', { name: 'Settings' })).toHaveAttribute(
      'data-presentation-mode',
      'full',
    );
    expect(screen.getByRole('toolbar')).toBeVisible();
    expect(screen.getByRole('status')).toHaveTextContent('Saving Settings');
  });
});
