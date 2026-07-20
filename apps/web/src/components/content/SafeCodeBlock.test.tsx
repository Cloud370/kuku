import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SafeCodeBlock } from './SafeCodeBlock';

afterEach(() => {
  cleanup();
});

describe('SafeCodeBlock', () => {
  it('bounds long code and highlights only registered languages', () => {
    const { rerender } = render(
      <SafeCodeBlock code={'fn main() {\n}'.repeat(200)} language="rust" />,
    );

    const region = screen.getByRole('region', { name: 'rust code' });
    expect(region).toHaveClass('max-h-96', 'overflow-auto');
    expect(region.querySelector('.hljs-keyword')).not.toBeNull();

    rerender(<SafeCodeBlock code={'<script>alert(1)</script>'} language="not-registered" />);
    expect(document.querySelector('script')).toBeNull();
    expect(screen.getByText('<script>alert(1)</script>')).toBeVisible();
  });

  it('copies the source without inserting it as HTML', async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    render(<SafeCodeBlock code="const safe = true;" language="typescript" />);

    await user.click(screen.getByRole('button', { name: 'Copy code' }));

    expect(writeText).toHaveBeenCalledWith('const safe = true;');
    expect(screen.getByRole('status')).toHaveTextContent('Copied');
  });
});
