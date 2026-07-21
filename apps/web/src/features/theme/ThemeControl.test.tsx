import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ThemeControl } from './ThemeControl';
import { resolveTheme } from './themePreference';

afterEach(cleanup);

describe('theme preference', () => {
  it('follows the system only while preference is system', () => {
    expect(resolveTheme({ preference: 'system', system: 'light' })).toBe('light');
    expect(resolveTheme({ preference: 'dark', system: 'light' })).toBe('dark');
  });

  it('persists only an explicit preference and applies it to the document', async () => {
    const user = userEvent.setup();
    const storage = { getItem: vi.fn().mockReturnValue('system'), setItem: vi.fn() };
    window.matchMedia = vi.fn().mockReturnValue({
      addEventListener: vi.fn(),
      matches: false,
      removeEventListener: vi.fn(),
    });
    render(<ThemeControl storage={storage} />);

    await user.click(screen.getByRole('button', { name: 'Dark theme' }));
    expect(storage.setItem).toHaveBeenCalledWith('kuku.theme.preference', 'dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
  });

  it('does not subscribe to system changes for an explicit preference', () => {
    window.matchMedia = vi.fn();
    render(
      <ThemeControl storage={{ getItem: vi.fn().mockReturnValue('dark'), setItem: vi.fn() }} />,
    );

    expect(window.matchMedia).not.toHaveBeenCalled();
    expect(document.documentElement.dataset.theme).toBe('dark');
  });
});
