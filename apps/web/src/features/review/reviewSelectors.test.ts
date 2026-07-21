import { describe, expect, it } from 'vitest';
import type { DiffDocument, FileContent, ReviewSnapshot } from '@/api/generated';

import { excerptAt, selectChanges } from './reviewSelectors';

const revision = 'a'.repeat(64);

function snapshot(availability: ReviewSnapshot['availability']): ReviewSnapshot {
  return {
    api_version: 1,
    availability,
    entries: [],
    next_cursor: null,
    revision,
    workspace_id: 'wsp_000000000000000000000001',
  };
}

describe('review selectors', () => {
  it('keeps unavailable and empty Changes states distinct', () => {
    expect(selectChanges(snapshot('not_git_repository'))).toEqual({
      kind: 'unavailable',
      reason: 'not_git_repository',
    });
    expect(selectChanges(snapshot('available'))).toEqual({ kind: 'empty' });
  });

  it('extracts a file-side excerpt by absolute line number', () => {
    const file: FileContent = {
      api_version: 1,
      binary: false,
      end_line: 12,
      next_start_line: null,
      path: 'src/lib.rs',
      revision,
      start_line: 10,
      text: 'ten\neleven\ntwelve',
      total_lines: 12,
      truncated: false,
      workspace_id: 'wsp_000000000000000000000001',
    };

    expect(excerptAt(file, 'file', 11, 12)).toBe('eleven\ntwelve');
    expect(excerptAt(file, 'old', 11, 12)).toBeNull();
  });

  it('extracts old and new excerpts without crossing sides', () => {
    const diff: DiffDocument = {
      api_version: 1,
      binary: false,
      hunks: [
        {
          lines: [
            { kind: 'deletion', old_line: 18, new_line: null, text: 'const old = 1;' },
            { kind: 'addition', old_line: null, new_line: 18, text: 'const next = 1;' },
            { kind: 'addition', old_line: null, new_line: 19, text: 'return next;' },
          ],
          new_lines: 2,
          new_start: 18,
          old_lines: 1,
          old_start: 18,
        },
      ],
      next_cursor: null,
      old_path: null,
      path: 'src/lib.ts',
      revision,
      truncated: false,
      workspace_id: 'wsp_000000000000000000000001',
    };

    expect(excerptAt(diff, 'old', 18, 18)).toBe('const old = 1;');
    expect(excerptAt(diff, 'new', 18, 19)).toBe('const next = 1;\nreturn next;');
  });
});
