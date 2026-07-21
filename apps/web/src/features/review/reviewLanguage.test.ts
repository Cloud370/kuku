import { describe, expect, it } from 'vitest';

import { resolveReviewLanguage } from './reviewLanguage';

describe('resolveReviewLanguage', () => {
  it('maps known extensions to the shared highlighter languages', () => {
    expect(resolveReviewLanguage('src/main.rs')).toBe('rust');
    expect(resolveReviewLanguage('scripts/check.sh')).toBe('bash');
    expect(resolveReviewLanguage('config.json')).toBe('json');
  });

  it('uses plaintext for unknown or missing languages', () => {
    expect(resolveReviewLanguage('artifact.made-up-lang')).toBe('plaintext');
    expect(resolveReviewLanguage(null)).toBe('plaintext');
  });
});
