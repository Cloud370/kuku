import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ExactRequestDialog } from './ExactRequestDialog';
import { contextFixture } from './testFixtures';

afterEach(cleanup);

describe('ExactRequestDialog', () => {
  function renderDialog() {
    const fixture = contextFixture();
    if (fixture.exact_request === null) throw new Error('exact Request fixture is missing');
    render(
      <ExactRequestDialog
        exactPayloadHash={fixture.exact_payload_hash}
        exactRequest={fixture.exact_request}
        onClose={vi.fn()}
        open
      />,
    );
  }

  it('surfaces request metadata before the raw payload', () => {
    renderDialog();

    expect(screen.getByLabelText('Request metadata')).toHaveTextContent('claude-fixture');
    expect(screen.getByLabelText('Request metadata')).toHaveTextContent('Streaming');
    expect(screen.getByLabelText('Request metadata')).toHaveTextContent('2 messages');
    expect(screen.getByLabelText('Request metadata')).toHaveTextContent('1 tool');
  });

  it('organizes the payload into collapsible debug sections', async () => {
    const user = userEvent.setup();
    renderDialog();

    const parameters = screen.getByRole('group', { name: 'Parameters' });
    expect(parameters).toBeVisible();
    expect(screen.getByRole('group', { name: 'Messages' })).toBeVisible();
    expect(screen.getByRole('group', { name: 'Tools' })).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Collapse Parameters' }));
    expect(screen.queryByText('max_output_tokens')).toBeNull();
    expect(screen.getByRole('button', { name: 'Expand Parameters' })).toBeVisible();
  });

  it('labels message roles and content block kinds for fast scanning', () => {
    renderDialog();

    const messages = screen.getByRole('group', { name: 'Messages' });
    expect(within(messages).getByText('system')).toBeVisible();
    expect(within(messages).getByText('user')).toBeVisible();
    expect(within(messages).getAllByText('text')).toHaveLength(4);
  });

  it('copies the complete exact request as JSON', async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    renderDialog();

    await user.click(screen.getByRole('button', { name: 'Copy request JSON' }));

    expect(writeText).toHaveBeenCalledWith(expect.stringContaining('"claude-fixture"'));
    expect(screen.getByRole('status')).toHaveTextContent('Request JSON copied');
  });
});
