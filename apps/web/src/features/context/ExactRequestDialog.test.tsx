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
        usage={fixture.usage.this_request}
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

  it('surfaces request usage next to the exact payload', () => {
    renderDialog();

    const usage = screen.getByLabelText('Request usage');
    expect(usage).toHaveTextContent('40%');
    expect(usage).toHaveTextContent('100');
    expect(usage).toHaveTextContent('20');
    expect(usage).toHaveTextContent('1.2s');
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

  it('provides a compact message navigator and focused message detail', async () => {
    const user = userEvent.setup();
    renderDialog();

    const messages = screen.getByRole('group', { name: 'Messages' });
    const navigator = within(messages).getByRole('navigation', { name: 'Message navigator' });
    expect(
      within(navigator).getByRole('button', { name: 'Select message 1 system' }),
    ).toBeVisible();
    expect(within(navigator).getByRole('button', { name: 'Select message 2 user' })).toBeVisible();
    expect(within(messages).getByRole('region', { name: 'Message 1 details' })).toHaveTextContent(
      'System prompt',
    );

    await user.click(within(navigator).getByRole('button', { name: 'Select message 2 user' }));
    expect(within(messages).getByRole('region', { name: 'Message 2 details' })).toHaveTextContent(
      'Review this change',
    );
    expect(within(messages).queryByText('System prompt')).toBeNull();
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
