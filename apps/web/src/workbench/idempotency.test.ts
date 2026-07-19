import { describe, expect, it } from 'vitest';

import { newIdempotencyKey } from './idempotency';

describe('newIdempotencyKey', () => {
  it('creates distinct opaque keys without inventing a branded API identifier', () => {
    const first = newIdempotencyKey();
    const second = newIdempotencyKey();

    expect(first).toMatch(
      /^idem-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    expect(second).toMatch(/^idem-[0-9a-f-]{36}$/);
    expect(second).not.toBe(first);
  });
});
