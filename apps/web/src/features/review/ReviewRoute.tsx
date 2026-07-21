import { ArrowLeft, Columns2, Rows3 } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import type {
  AnnotationDraft,
  ChangeEntry,
  DiffDocument,
  FileContent,
  TaskId,
  WorkspaceId,
} from '@/api/generated';
import { webApi } from '../../api/client';
import { SafeCodeBlock } from '../../components/content/SafeCodeBlock';

import { ChangeList } from './ChangeList';
import { DiffViewer, type DiffView } from './DiffViewer';
import { FileSearch } from './FileSearch';
import { FileViewer } from './FileViewer';
import { resolveReviewLanguage } from './reviewLanguage';
import { ReviewModeTabs, type ReviewMode } from './ReviewModeTabs';

type ReviewOperation = 'changes' | 'diff' | 'file' | 'search' | 'tree';
export type ReviewDataSource = {
  -readonly [Operation in ReviewOperation]: (typeof webApi.review)[Operation];
};

type AsyncState<T> =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'error' }
  | { kind: 'ready'; value: T };

export interface ReviewRouteProps {
  taskId: TaskId;
  workspaceId: WorkspaceId;
  mode: ReviewMode;
  presentation: 'full' | 'embedded';
  onDraftRange: (anchor: AnnotationDraft) => void;
  onLeave: () => void;
  dataSource?: ReviewDataSource;
  initialPath?: string | null;
}

export function ReviewCode({ source, language }: { source: string; language: string | null }) {
  return <SafeCodeBlock code={source} language={resolveReviewLanguage(language)} />;
}

export function ReviewRoute({
  dataSource = webApi.review,
  initialPath = null,
  mode,
  onDraftRange,
  onLeave,
  presentation,
  taskId,
  workspaceId,
}: ReviewRouteProps) {
  const [activeMode, setActiveMode] = useState(mode);
  const [changesState, setChangesState] = useState<
    AsyncState<Awaited<ReturnType<ReviewDataSource['changes']>>>
  >({ kind: 'idle' });
  const [fileState, setFileState] = useState<AsyncState<FileContent>>({ kind: 'idle' });
  const [diffState, setDiffState] = useState<AsyncState<DiffDocument>>({ kind: 'idle' });
  const [diffView, setDiffView] = useState<DiffView>('unified');

  const openFile = useCallback(
    async (path: string) => {
      setFileState({ kind: 'loading' });
      try {
        const value = await dataSource.file(workspaceId, { end_line: 2000, path, start_line: 1 });
        setFileState({ kind: 'ready', value });
      } catch {
        setFileState({ kind: 'error' });
      }
    },
    [dataSource, workspaceId],
  );

  useEffect(() => {
    setActiveMode(mode);
  }, [mode]);

  useEffect(() => {
    if (activeMode !== 'changes') return;
    let current = true;
    setChangesState({ kind: 'loading' });
    void dataSource
      .changes(workspaceId, { cursor: null, limit: 100 })
      .then((value) => {
        if (current) setChangesState({ kind: 'ready', value });
      })
      .catch(() => {
        if (current) setChangesState({ kind: 'error' });
      });
    return () => {
      current = false;
    };
  }, [activeMode, dataSource, workspaceId]);

  useEffect(() => {
    if (initialPath === null) return;
    void openFile(initialPath);
  }, [initialPath, openFile]);

  async function openDiff(entry: ChangeEntry) {
    setDiffState({ kind: 'loading' });
    try {
      const value = await dataSource.diff(workspaceId, {
        cursor: null,
        limit: 4000,
        path: entry.path,
        revision: entry.revision,
      });
      setDiffState({ kind: 'ready', value });
    } catch {
      setDiffState({ kind: 'error' });
    }
  }

  async function loadMoreChanges(cursor: string) {
    if (changesState.kind !== 'ready') return;
    try {
      const page = await dataSource.changes(workspaceId, { cursor, limit: 100 });
      if (page.revision !== changesState.value.revision) {
        setChangesState({ kind: 'error' });
        return;
      }
      setChangesState({
        kind: 'ready',
        value: {
          ...changesState.value,
          entries: [...changesState.value.entries, ...page.entries],
          next_cursor: page.next_cursor,
        },
      });
    } catch {
      setChangesState({ kind: 'error' });
    }
  }

  async function loadMoreFile(nextStartLine: number) {
    if (fileState.kind !== 'ready') return;
    try {
      const page = await dataSource.file(workspaceId, {
        end_line: nextStartLine + 1999,
        path: fileState.value.path,
        start_line: nextStartLine,
      });
      if (page.revision !== fileState.value.revision || page.text === null) {
        setFileState({ kind: 'error' });
        return;
      }
      setFileState({
        kind: 'ready',
        value: {
          ...fileState.value,
          end_line: page.end_line,
          next_start_line: page.next_start_line,
          text: `${fileState.value.text ?? ''}\n${page.text}`,
          total_lines: page.total_lines,
          truncated: page.truncated,
        },
      });
    } catch {
      setFileState({ kind: 'error' });
    }
  }

  async function loadMoreDiff(cursor: string) {
    if (diffState.kind !== 'ready') return;
    try {
      const page = await dataSource.diff(workspaceId, {
        cursor,
        limit: 4000,
        path: diffState.value.path,
        revision: diffState.value.revision,
      });
      if (page.revision !== diffState.value.revision) {
        setDiffState({ kind: 'error' });
        return;
      }
      setDiffState({
        kind: 'ready',
        value: {
          ...diffState.value,
          hunks: [...diffState.value.hunks, ...page.hunks],
          next_cursor: page.next_cursor,
          truncated: page.truncated,
        },
      });
    } catch {
      setDiffState({ kind: 'error' });
    }
  }

  return (
    <section
      aria-label="Review"
      className="grid h-full min-h-0 min-w-0 grid-rows-[52px_minmax(0,1fr)] bg-[var(--color-surface)] text-[var(--color-text-primary)]"
      data-presentation={presentation}
      data-task-id={taskId}
    >
      <header className="flex min-w-0 items-center gap-2 border-b border-[var(--color-border)] px-2">
        <button
          aria-label="Leave Review"
          className="inline-flex size-8 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)]"
          onClick={onLeave}
          title="Leave Review"
          type="button"
        >
          <ArrowLeft aria-hidden="true" size={16} />
        </button>
        <ReviewModeTabs mode={activeMode} onChange={setActiveMode} />
        {activeMode === 'changes' && diffState.kind === 'ready' ? (
          <div aria-label="Diff layout" className="ml-auto flex" role="group">
            <button
              aria-label="Unified diff"
              onClick={() => {
                setDiffView('unified');
              }}
              title="Unified diff"
              type="button"
            >
              <Rows3 aria-hidden="true" size={16} />
            </button>
            <button
              aria-label="Split diff"
              onClick={() => {
                setDiffView('split');
              }}
              title="Split diff"
              type="button"
            >
              <Columns2 aria-hidden="true" size={16} />
            </button>
          </div>
        ) : null}
      </header>
      {activeMode === 'files' ? (
        <div className="grid min-h-0 min-w-0 grid-cols-[minmax(12rem,18rem)_minmax(0,1fr)] max-[600px]:grid-cols-1">
          <FileSearch
            dataSource={dataSource}
            onSelect={(path) => {
              void openFile(path);
            }}
            workspaceId={workspaceId}
          />
          <div className="min-h-0 min-w-0 overflow-auto">
            {fileState.kind === 'idle' ? (
              <p className="grid min-h-48 place-items-center text-sm">Select a file</p>
            ) : null}
            {fileState.kind === 'loading' ? (
              <p className="p-4" role="status">
                Loading file
              </p>
            ) : null}
            {fileState.kind === 'error' ? (
              <p className="p-4" role="alert">
                Unable to load file
              </p>
            ) : null}
            {fileState.kind === 'ready' ? (
              <FileViewer
                content={fileState.value}
                onDraftRange={onDraftRange}
                onLoadMore={(nextStartLine) => {
                  void loadMoreFile(nextStartLine);
                }}
              />
            ) : null}
          </div>
        </div>
      ) : (
        <div className="grid min-h-0 min-w-0 grid-cols-[minmax(14rem,22rem)_minmax(0,1fr)] max-[700px]:grid-cols-1">
          <div className="min-h-0 overflow-auto border-r border-[var(--color-border)]">
            {changesState.kind === 'loading' || changesState.kind === 'idle' ? (
              <p className="p-4" role="status">
                Loading changes
              </p>
            ) : null}
            {changesState.kind === 'error' ? (
              <p className="p-4" role="alert">
                Unable to load changes
              </p>
            ) : null}
            {changesState.kind === 'ready' ? (
              <ChangeList
                onLoadMore={(cursor) => {
                  void loadMoreChanges(cursor);
                }}
                onSelect={(entry) => {
                  void openDiff(entry);
                }}
                snapshot={changesState.value}
              />
            ) : null}
          </div>
          <div className="min-h-0 min-w-0 overflow-auto">
            {diffState.kind === 'idle' ? (
              <p className="grid min-h-48 place-items-center text-sm">Select a changed file</p>
            ) : null}
            {diffState.kind === 'loading' ? (
              <p className="p-4" role="status">
                Loading diff
              </p>
            ) : null}
            {diffState.kind === 'error' ? (
              <p className="p-4" role="alert">
                Unable to load diff
              </p>
            ) : null}
            {diffState.kind === 'ready' ? (
              <DiffViewer
                document={diffState.value}
                onDraftRange={onDraftRange}
                onLoadMore={(cursor) => {
                  void loadMoreDiff(cursor);
                }}
                view={diffView}
              />
            ) : null}
          </div>
        </div>
      )}
    </section>
  );
}
