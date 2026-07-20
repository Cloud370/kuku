import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { activateFocusTrap } from './focusTrap';

afterEach(() => {
  document.body.innerHTML = '';
});

describe('activateFocusTrap', () => {
  it('cycles Tab inside the surface, closes on Escape, and restores focus', () => {
    const trigger = document.createElement('button');
    const surface = document.createElement('div');
    const first = document.createElement('button');
    const last = document.createElement('button');
    const onEscape = vi.fn();
    surface.append(first, last);
    document.body.append(trigger, surface);
    trigger.focus();

    const deactivate = activateFocusTrap(surface, onEscape, trigger);
    expect(first).toHaveFocus();
    last.focus();
    document.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Tab' }));
    expect(first).toHaveFocus();
    document.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }));
    expect(onEscape).toHaveBeenCalledTimes(1);

    deactivate();
    expect(trigger).toHaveFocus();
  });
});
