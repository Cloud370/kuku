import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import { SafeMarkdown } from './SafeMarkdown';

afterEach(() => {
  cleanup();
});

describe('SafeMarkdown', () => {
  it('removes executable HTML and unsafe links while retaining Markdown', () => {
    render(<SafeMarkdown source={'[bad](javascript:alert(1))<script>x</script>**ok**'} />);

    expect(document.querySelector('script')).toBeNull();
    expect(screen.getByText('ok')).toBeVisible();
    expect(screen.getByText('bad').closest('a')).not.toHaveAttribute('href');
  });

  it('adds safe external-link protections without changing relative links', () => {
    render(<SafeMarkdown source={'[external](https://example.com) [local](/docs)'} />);

    expect(screen.getByRole('link', { name: 'external' })).toHaveAttribute(
      'rel',
      'noopener noreferrer',
    );
    expect(screen.getByRole('link', { name: 'external' })).toHaveAttribute('target', '_blank');
    expect(screen.getByRole('link', { name: 'local' })).not.toHaveAttribute('target');
  });

  it('does not load remote media from untrusted Markdown', () => {
    render(<SafeMarkdown source={'![tracking](https://example.com/pixel.png)'} />);

    expect(document.querySelector('img')).toBeNull();
  });

  it('delegates fenced code to the bounded safe code block', () => {
    render(<SafeMarkdown source={'Before\n\n```rust\nfn main() {}\n```\n\nAfter'} />);

    expect(screen.getByText('Before')).toBeVisible();
    expect(screen.getByRole('region', { name: 'rust code' })).toBeVisible();
    expect(screen.getByText('After')).toBeVisible();
  });
});
