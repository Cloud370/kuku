import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { RequestLinks } from './RequestLinks';
import { requestOne, requestTwo } from './testFixtures';

afterEach(cleanup);

describe('RequestLinks', () => {
  it('renders scannable provider Request links and reveals IDs on demand', async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<RequestLinks onSelect={onSelect} requestIds={[requestOne, requestTwo]} />);

    const links = screen.getAllByRole('button', { name: /select request \d/i });
    const secondLink = links.at(1);
    if (secondLink === undefined) throw new Error('second Request link is missing');
    expect(links).toHaveLength(2);
    expect(links.map((link) => link.textContent)).toEqual(['Request 1', 'Request 2']);
    expect(screen.queryByText(requestOne)).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Show Request 1 details' }));
    expect(screen.getByText(requestOne)).toBeVisible();
    await user.click(secondLink);
    expect(onSelect).toHaveBeenCalledWith(requestTwo);
  });
});
