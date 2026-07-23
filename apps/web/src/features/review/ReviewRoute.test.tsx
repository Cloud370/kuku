import '@testing-library/jest-dom/vitest';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AnnotationDraft, DiffDocument, FileContent, ReviewSnapshot } from '@/api/generated';
import { WebApiError } from '../../api/client';

import { DiffViewer } from './DiffViewer';
import { ReviewCode, ReviewRoute, type ReviewDataSource } from './ReviewRoute';

const workspaceId = 'wsp_000000000000000000000001';
const taskId = 'tsk_000000000000000000000001';
const revision = 'a'.repeat(64);

function changes(availability: ReviewSnapshot['availability'] = 'available'): ReviewSnapshot {
  return {
    api_version: 1,
    availability,
    entries: [],
    next_cursor: null,
    revision,
    workspace_id: workspaceId,
  };
}

function diff(): DiffDocument {
  return {
    api_version: 1,
    binary: false,
    hunks: [
      {
        lines: [
          { kind: 'addition', new_line: 18, old_line: null, text: 'const next = 1;' },
          { kind: 'addition', new_line: 19, old_line: null, text: 'return next;' },
          { kind: 'addition', new_line: 20, old_line: null, text: '}' },
        ],
        new_lines: 3,
        new_start: 18,
        old_lines: 0,
        old_start: 18,
      },
    ],
    next_cursor: null,
    old_path: null,
    path: 'src/lib.ts',
    revision,
    truncated: false,
    workspace_id: workspaceId,
  };
}

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function fileContent(
  path: string,
  text: string,
  options: { nextStartLine?: number | null; startLine?: number } = {},
): FileContent {
  const startLine = options.startLine ?? 1;
  return {
    api_version: 1,
    binary: false,
    end_line: startLine,
    next_start_line: options.nextStartLine ?? null,
    path,
    revision,
    start_line: startLine,
    text,
    total_lines: options.nextStartLine === undefined ? 1 : 2,
    truncated: options.nextStartLine !== undefined && options.nextStartLine !== null,
    workspace_id: workspaceId,
  };
}

function diffDocument(path: string, text: string, line: number): DiffDocument {
  const document = diff();
  document.path = path;
  document.hunks = [
    {
      lines: [{ kind: 'addition', new_line: line, old_line: null, text }],
      new_lines: 1,
      new_start: line,
      old_lines: 0,
      old_start: line,
    },
  ];
  return document;
}

function source(snapshot = changes()): ReviewDataSource {
  return {
    changes: vi.fn().mockResolvedValue(snapshot),
    diff: vi.fn().mockResolvedValue(diff()),
    file: vi.fn().mockResolvedValue({
      api_version: 1,
      binary: false,
      end_line: 2,
      next_start_line: null,
      path: 'src/lib.ts',
      revision,
      start_line: 1,
      text: 'one\ntwo',
      total_lines: 2,
      truncated: false,
      workspace_id: workspaceId,
    } satisfies FileContent),
    search: vi.fn().mockResolvedValue({
      api_version: 1,
      matches: [],
      next_cursor: null,
      revision,
      workspace_id: workspaceId,
    }),
    tree: vi.fn().mockResolvedValue({
      api_version: 1,
      entries: [],
      next_cursor: null,
      revision,
      workspace_id: workspaceId,
    }),
  };
}

function serverBusy(): WebApiError {
  return new WebApiError(503, {
    api_version: 1,
    code: 'server_busy',
    details: null,
    message: 'review capacity is busy',
    trace_id: 'review-service',
  });
}

afterEach(cleanup);

describe('ReviewRoute', () => {
  it('accepts per-file revisions that differ from the aggregate snapshot revision', async () => {
    const snapshot = changes();
    snapshot.entries = [
      {
        additions: 1,
        binary: false,
        deletions: 1,
        kind: 'modified',
        old_path: null,
        path: 'src/main.ts',
        revision: 'b'.repeat(64),
        staged: false,
        worktree: true,
      },
    ];

    render(
      <ReviewRoute
        dataSource={source(snapshot)}
        mode="changes"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    expect(await screen.findByLabelText('Workspace changes')).toHaveTextContent('src/main.ts');
  });

  it('keeps Files available when Changes is unavailable', async () => {
    const api = source(changes('not_git_repository'));
    render(
      <ReviewRoute
        dataSource={api}
        mode="changes"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    expect(await screen.findByText('Changes unavailable for this workspace')).toBeVisible();
    expect(screen.getByRole('tab', { name: 'Files' })).toBeEnabled();
    await userEvent.click(screen.getByRole('tab', { name: 'Files' }));
    expect(await screen.findByRole('searchbox', { name: 'Find a file' })).toBeVisible();
  });

  it('renders explicit loading, error, and empty Changes states', async () => {
    let resolveChanges: ((value: ReviewSnapshot) => void) | undefined;
    const api = source();
    api.changes = vi.fn().mockReturnValue(
      new Promise<ReviewSnapshot>((resolve) => {
        resolveChanges = resolve;
      }),
    );
    const view = render(
      <ReviewRoute
        dataSource={api}
        mode="changes"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="embedded"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );
    expect(screen.getByRole('status')).toHaveTextContent('Loading changes');
    resolveChanges?.(changes());
    expect(await screen.findByText('No workspace changes')).toBeVisible();

    api.changes = vi.fn().mockRejectedValue(new Error('offline'));
    view.rerender(
      <ReviewRoute
        dataSource={api}
        mode="files"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="embedded"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );
    await userEvent.click(screen.getByRole('tab', { name: 'Changes' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to load changes');
  });

  it('retries a transient busy response while loading a diff', async () => {
    const snapshot = changes();
    snapshot.entries = [
      {
        additions: 3,
        binary: false,
        deletions: 0,
        kind: 'modified',
        old_path: null,
        path: 'src/lib.ts',
        revision,
        staged: false,
        worktree: true,
      },
    ];
    const api = source(snapshot);
    api.diff = vi.fn().mockRejectedValueOnce(serverBusy()).mockResolvedValueOnce(diff());
    render(
      <ReviewRoute
        dataSource={api}
        mode="changes"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    const changesList = await screen.findByLabelText('Workspace changes');
    await userEvent.click(within(changesList).getByRole('button'));

    expect(await screen.findByRole('button', { name: 'Select new line 18' })).toBeVisible();
    expect(api.diff).toHaveBeenCalledTimes(2);
  });

  it('keeps the latest selected file when an older request finishes last', async () => {
    const firstRequest = deferred<FileContent>();
    const secondRequest = deferred<FileContent>();
    const api = source();
    api.tree = vi.fn().mockResolvedValue({
      api_version: 1,
      entries: ['first', 'second'].map((name) => ({
        binary: false,
        change: null,
        kind: 'file' as const,
        name: `${name}.ts`,
        path: `src/${name}.ts`,
        revision,
        size_bytes: 10,
      })),
      next_cursor: null,
      revision,
      workspace_id: workspaceId,
    });
    api.file = vi
      .fn<ReviewDataSource['file']>()
      .mockReturnValueOnce(firstRequest.promise)
      .mockReturnValueOnce(secondRequest.promise);
    const onDraftRange = vi.fn<(anchor: AnnotationDraft) => void>();
    render(
      <ReviewRoute
        dataSource={api}
        mode="files"
        onDraftRange={onDraftRange}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    await userEvent.click(await screen.findByRole('button', { name: 'src/first.ts' }));
    await userEvent.click(screen.getByRole('button', { name: 'src/second.ts' }));
    await act(async () => {
      secondRequest.resolve(fileContent('src/second.ts', 'second current'));
      await secondRequest.promise;
    });
    expect(screen.getByText('second current')).toBeVisible();

    await act(async () => {
      firstRequest.resolve(fileContent('src/first.ts', 'first stale'));
      await firstRequest.promise;
    });
    expect(screen.queryByText('first stale')).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Add annotation' }));
    expect(onDraftRange).toHaveBeenLastCalledWith(
      expect.objectContaining({ excerpt: 'second current', path: 'src/second.ts' }),
    );
  });

  it('keeps the latest selected diff when an older request finishes last', async () => {
    const snapshot = changes();
    snapshot.entries = ['first', 'second'].map((name) => ({
      additions: 1,
      binary: false,
      deletions: 0,
      kind: 'modified' as const,
      old_path: null,
      path: `src/${name}.ts`,
      revision,
      staged: false,
      worktree: true,
    }));
    const firstRequest = deferred<DiffDocument>();
    const secondRequest = deferred<DiffDocument>();
    const api = source(snapshot);
    api.diff = vi
      .fn<ReviewDataSource['diff']>()
      .mockReturnValueOnce(firstRequest.promise)
      .mockReturnValueOnce(secondRequest.promise);
    const onDraftRange = vi.fn<(anchor: AnnotationDraft) => void>();
    render(
      <ReviewRoute
        dataSource={api}
        mode="changes"
        onDraftRange={onDraftRange}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    const changesList = await screen.findByLabelText('Workspace changes');
    await userEvent.click(within(changesList).getByRole('button', { name: /src\/first\.ts/ }));
    await userEvent.click(within(changesList).getByRole('button', { name: /src\/second\.ts/ }));
    await act(async () => {
      secondRequest.resolve(diffDocument('src/second.ts', 'second diff', 22));
      await secondRequest.promise;
    });
    expect(screen.getByText('second diff')).toBeVisible();

    await act(async () => {
      firstRequest.resolve(diffDocument('src/first.ts', 'first stale diff', 11));
      await firstRequest.promise;
    });
    expect(screen.queryByText('first stale diff')).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Select new line 22' }));
    await userEvent.click(screen.getByRole('button', { name: 'Select new line 22' }));
    expect(onDraftRange).toHaveBeenLastCalledWith(
      expect.objectContaining({ excerpt: 'second diff', path: 'src/second.ts' }),
    );
  });

  it('does not append a stale file page after another file is selected', async () => {
    const pageRequest = deferred<FileContent>();
    const api = source();
    api.tree = vi.fn().mockResolvedValue({
      api_version: 1,
      entries: ['first', 'second'].map((name) => ({
        binary: false,
        change: null,
        kind: 'file' as const,
        name: `${name}.ts`,
        path: `src/${name}.ts`,
        revision,
        size_bytes: 10,
      })),
      next_cursor: null,
      revision,
      workspace_id: workspaceId,
    });
    api.file = vi.fn<ReviewDataSource['file']>().mockImplementation((_workspace, query) => {
      if (query.path === 'src/first.ts' && query.start_line === 1) {
        return Promise.resolve(fileContent('src/first.ts', 'first head', { nextStartLine: 2 }));
      }
      if (query.path === 'src/first.ts') return pageRequest.promise;
      return Promise.resolve(fileContent('src/second.ts', 'second current'));
    });
    const onDraftRange = vi.fn<(anchor: AnnotationDraft) => void>();
    render(
      <ReviewRoute
        dataSource={api}
        mode="files"
        onDraftRange={onDraftRange}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    await userEvent.click(await screen.findByRole('button', { name: 'src/first.ts' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Load more file content' }));
    await userEvent.click(screen.getByRole('button', { name: 'src/second.ts' }));
    expect(await screen.findByText('second current')).toBeVisible();
    await act(async () => {
      pageRequest.resolve(fileContent('src/first.ts', 'first stale tail', { startLine: 2 }));
      await pageRequest.promise;
    });

    expect(screen.queryByText(/first stale tail/)).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Add annotation' }));
    expect(onDraftRange).toHaveBeenLastCalledWith(
      expect.objectContaining({ excerpt: 'second current', path: 'src/second.ts' }),
    );
  });

  it('does not append a stale diff page after another diff is selected', async () => {
    const snapshot = changes();
    snapshot.entries = ['first', 'second'].map((name) => ({
      additions: 1,
      binary: false,
      deletions: 0,
      kind: 'modified' as const,
      old_path: null,
      path: `src/${name}.ts`,
      revision,
      staged: false,
      worktree: true,
    }));
    const pageRequest = deferred<DiffDocument>();
    const api = source(snapshot);
    api.diff = vi.fn<ReviewDataSource['diff']>().mockImplementation((_workspace, query) => {
      if (query.path === 'src/first.ts' && query.cursor === null) {
        return Promise.resolve({
          ...diffDocument('src/first.ts', 'first head', 1),
          next_cursor: 'first-next',
          truncated: true,
        });
      }
      if (query.path === 'src/first.ts') return pageRequest.promise;
      return Promise.resolve(diffDocument('src/second.ts', 'second current diff', 20));
    });
    render(
      <ReviewRoute
        dataSource={api}
        mode="changes"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    const changesList = await screen.findByLabelText('Workspace changes');
    await userEvent.click(within(changesList).getByRole('button', { name: /src\/first\.ts/ }));
    await userEvent.click(await screen.findByRole('button', { name: 'Load more diff' }));
    await userEvent.click(within(changesList).getByRole('button', { name: /src\/second\.ts/ }));
    expect(await screen.findByText('second current diff')).toBeVisible();
    await act(async () => {
      pageRequest.resolve(diffDocument('src/first.ts', 'first stale diff page', 2));
      await pageRequest.promise;
    });

    expect(screen.queryByText('first stale diff page')).toBeNull();
    expect(screen.getByText('second current diff')).toBeVisible();
  });

  it('rejects file content whose identity does not match the request', async () => {
    const api = source();
    api.file = vi.fn().mockResolvedValue(fileContent('src/wrong.ts', 'wrong document'));
    render(
      <ReviewRoute
        dataSource={api}
        initialPath="src/expected.ts"
        mode="files"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );

    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to load file');
    expect(screen.queryByRole('button', { name: 'Add annotation' })).toBeNull();
  });

  it('resets the selected document when the task changes', async () => {
    const api = source();
    const view = render(
      <ReviewRoute
        dataSource={api}
        initialPath="src/lib.ts"
        mode="files"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="full"
        taskId={taskId}
        workspaceId={workspaceId}
      />,
    );
    expect(await screen.findByRole('button', { name: 'Add annotation' })).toBeVisible();

    view.rerender(
      <ReviewRoute
        dataSource={api}
        mode="files"
        onDraftRange={vi.fn()}
        onLeave={vi.fn()}
        presentation="full"
        taskId="tsk_000000000000000000000002"
        workspaceId={workspaceId}
      />,
    );

    await waitFor(() => {
      expect(screen.queryByRole('button', { name: 'Add annotation' })).toBeNull();
      expect(screen.getByText('Select a file')).toBeVisible();
    });
  });
});

describe('safe Review rendering', () => {
  it('anchors a selected diff range to the new side', async () => {
    const onDraftRange = vi.fn<(anchor: AnnotationDraft) => void>();
    render(<DiffViewer document={diff()} onDraftRange={onDraftRange} view="unified" />);
    const user = userEvent.setup();

    await user.click(screen.getByRole('button', { name: 'Select new line 18' }));
    await user.click(screen.getByRole('button', { name: 'Select new line 20' }));

    expect(onDraftRange).toHaveBeenCalledWith({
      comment: '',
      end_line: 20,
      excerpt: 'const next = 1;\nreturn next;\n}',
      path: 'src/lib.ts',
      revision,
      side: 'new',
      start_line: 18,
    });
  });

  it('renders unknown language source as escaped plaintext', async () => {
    render(<ReviewCode language="made-up-lang" source="<script>alert(1)</script>" />);

    expect(screen.getByText('<script>alert(1)</script>')).toBeVisible();
    expect(document.querySelector('script')).toBeNull();
    await waitFor(() => expect(screen.getByRole('region')).toHaveAccessibleName('plaintext code'));
  });

  it('renders split diff as independent old and new side regions', () => {
    const document = diff();
    document.hunks[0]?.lines.unshift({
      kind: 'deletion',
      new_line: null,
      old_line: 18,
      text: 'const previous = 1;',
    });
    render(<DiffViewer document={document} onDraftRange={vi.fn()} view="split" />);

    expect(screen.getByRole('region', { name: 'Old side' })).toHaveTextContent(
      'const previous = 1;',
    );
    expect(screen.getByRole('region', { name: 'New side' })).toHaveTextContent('const next = 1;');
  });
});
