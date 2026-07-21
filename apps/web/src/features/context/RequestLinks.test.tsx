import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { RequestLinks } from './RequestLinks';
import { requestOne, requestTwo } from './testFixtures';

afterEach(cleanup);

describe('RequestLinks', () => {
  it('renders every provider Request in canonical order', async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<RequestLinks onSelect={onSelect} requestIds={[requestOne, requestTwo]} />);

    const links = screen.getAllByRole('button', { name: /request req_/i });
    const secondLink = links.at(1);
    if (secondLink === undefined) throw new Error('second Request link is missing');
    expect(links).toHaveLength(2);
    expect(links.map((link) => link.textContent)).toEqual([requestOne, requestTwo]);
    await user.click(secondLink);
    expect(onSelect).toHaveBeenCalledWith(requestTwo);
  });
});
