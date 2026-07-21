import type { Meta, StoryObj } from '@storybook/react';
import type { DiffDocument, FileContent, ReviewSnapshot } from '@/api/generated';

import { DiffViewer } from './DiffViewer';
import { ReviewRoute, type ReviewDataSource } from './ReviewRoute';

const workspaceId = 'wsp_000000000000000000000001';
const taskId = 'tsk_000000000000000000000001';
const revision = 'a'.repeat(64);
const noop = () => undefined;

const emptyChanges: ReviewSnapshot = {
  api_version: 1,
  availability: 'available',
  entries: [],
  next_cursor: null,
  revision,
  workspace_id: workspaceId,
};

const changed: ReviewSnapshot = {
  ...emptyChanges,
  entries: [
    {
      additions: 3,
      binary: false,
      deletions: 1,
      kind: 'modified',
      old_path: null,
      path: 'src/features/review/ReviewRoute.tsx',
      revision,
      staged: false,
      worktree: true,
    },
  ],
};

const textFile: FileContent = {
  api_version: 1,
  binary: false,
  end_line: 3,
  next_start_line: null,
  path: 'src/features/review/ReviewRoute.tsx',
  revision,
  start_line: 1,
  text: 'export function Review() {\n  return null;\n}',
  total_lines: 3,
  truncated: false,
  workspace_id: workspaceId,
};

const reviewDiff: DiffDocument = {
  api_version: 1,
  binary: false,
  hunks: [
    {
      lines: [
        { kind: 'deletion', new_line: null, old_line: 18, text: 'const state = oldValue;' },
        { kind: 'addition', new_line: 18, old_line: null, text: 'const state = nextValue;' },
        { kind: 'context', new_line: 19, old_line: 19, text: 'return state;' },
      ],
      new_lines: 2,
      new_start: 18,
      old_lines: 2,
      old_start: 18,
    },
  ],
  next_cursor: null,
  old_path: null,
  path: textFile.path,
  revision,
  truncated: false,
  workspace_id: workspaceId,
};

function source(overrides: Partial<ReviewDataSource> = {}): ReviewDataSource {
  return {
    changes: () => Promise.resolve(emptyChanges),
    diff: () => Promise.resolve(reviewDiff),
    file: () => Promise.resolve(textFile),
    search: () =>
      Promise.resolve({
        api_version: 1,
        matches: [],
        next_cursor: null,
        revision,
        workspace_id: workspaceId,
      }),
    tree: () =>
      Promise.resolve({
        api_version: 1,
        entries: [
          {
            binary: false,
            change: 'modified',
            kind: 'file',
            name: 'ReviewRoute.tsx',
            path: textFile.path,
            revision,
            size_bytes: textFile.text?.length ?? null,
          },
        ],
        next_cursor: null,
        revision,
        workspace_id: workspaceId,
      }),
    ...overrides,
  };
}

const meta: Meta<typeof ReviewRoute> = {
  component: ReviewRoute,
  title: 'Experience/Review',
  args: {
    dataSource: source(),
    mode: 'files',
    onDraftRange: noop,
    onLeave: noop,
    presentation: 'full',
    taskId,
    workspaceId,
  },
  parameters: { layout: 'fullscreen' },
};

export default meta;
type Story = StoryObj<typeof ReviewRoute>;

export const Loading: Story = {
  args: { dataSource: source({ changes: () => new Promise(() => undefined) }), mode: 'changes' },
};

export const Empty: Story = { args: { mode: 'changes' } };

export const Error: Story = {
  args: {
    dataSource: source({ changes: () => Promise.reject(new globalThis.Error('offline')) }),
    mode: 'changes',
  },
};

export const FilesOnly: Story = {
  args: {
    dataSource: source({
      changes: () => Promise.resolve({ ...emptyChanges, availability: 'not_git_repository' }),
    }),
    mode: 'changes',
  },
};

export const GitChanges: Story = {
  args: { dataSource: source({ changes: () => Promise.resolve(changed) }), mode: 'changes' },
};

export const Binary: Story = {
  args: {
    dataSource: source({
      file: () => Promise.resolve({ ...textFile, binary: true, text: null }),
    }),
    initialPath: 'assets/logo.bin',
  },
};

export const Deleted: Story = {
  render: () => (
    <DiffViewer
      document={{
        ...reviewDiff,
        hunks: [
          {
            lines: [{ kind: 'deletion', new_line: null, old_line: 1, text: 'removed' }],
            new_lines: 0,
            new_start: 1,
            old_lines: 1,
            old_start: 1,
          },
        ],
      }}
      onDraftRange={noop}
      view="unified"
    />
  ),
};

export const LongPath: Story = {
  args: {
    dataSource: source({
      file: () =>
        Promise.resolve({
          ...textFile,
          path: `${'very-long-directory/'.repeat(12)}ReviewRoute.tsx`,
        }),
    }),
    initialPath: textFile.path,
  },
};

export const UnifiedDiff: Story = {
  render: () => <DiffViewer document={reviewDiff} onDraftRange={noop} view="unified" />,
};

export const SplitDiff: Story = {
  render: () => <DiffViewer document={reviewDiff} onDraftRange={noop} view="split" />,
};
