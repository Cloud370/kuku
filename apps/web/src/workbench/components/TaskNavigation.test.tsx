import '@testing-library/jest-dom/vitest';

import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
  CreateTaskResponse,
  TaskPage,
  TaskProjection,
  TaskState,
  TaskSummary,
} from '../../api/generated';
import taskProjectionJson from '../../api/generated/fixtures/task_projection.json';
import { webApi } from '../../api/client';
import type { PendingCommand, PendingCommandResult } from '../workbenchStore';

import { NewTaskForm } from './NewTaskForm';
import {
  TaskNavigation,
  type TaskNavigationCommands,
  type TaskNavigationProps,
} from './TaskNavigation';

type WebApi = typeof webApi;

const workspaceId = 'wsp_000000000000000000000001';
const otherWorkspaceId = 'wsp_000000000000000000000002';
const taskId = 'tsk_000000000000000000000001';

afterEach(() => {
  cleanup();
});

function task(
  title: string,
  state: TaskState,
  id = `${taskId.slice(0, -1)}${String(title.length % 10)}`,
): TaskSummary {
  return {
    active_run_id: null,
    latest_run_id: null,
    state,
    task_id: id,
    title,
    updated_at: '2026-07-18T09:00:00Z',
    workspace_id: workspaceId,
  };
}

function projection(id = taskId): TaskProjection {
  const value = structuredClone(taskProjectionJson) as TaskProjection;
  value.task.task_id = id;
  value.task.workspace_id = workspaceId;
  return value;
}

function created(id = taskId): CreateTaskResponse {
  return { api_version: 1, projection: projection(id), replayed: false };
}

function taskPage(items: TaskSummary[], nextCursor: string | null = null): TaskPage {
  return { api_version: 1, items, next_cursor: nextCursor };
}

function setupApi() {
  const list = vi.fn<WebApi['tasks']['list']>();
  const workspaces = vi.fn<WebApi['workspaces']['list']>();
  workspaces.mockResolvedValue({
    api_version: 1,
    items: [
      {
        availability: 'available',
        branch: 'main',
        is_default: true,
        label: 'kuku',
        workspace_id: workspaceId,
      },
    ],
    server_revision: 'rev_1',
  });
  return {
    api: {
      ...webApi,
      tasks: { ...webApi.tasks, list },
      workspaces: { ...webApi.workspaces, list: workspaces },
    },
    list,
  };
}

function setupCommands() {
  return {
    createTask: vi.fn<(workspaceId: string) => Promise<CreateTaskResponse | undefined>>(),
    retryPendingCommand: vi.fn<() => Promise<PendingCommandResult>>(),
    abandonConflictedCommand: vi.fn<() => void>(),
  } satisfies TaskNavigationCommands;
}

function navigationProps(api: WebApi, commands: TaskNavigationCommands): TaskNavigationProps {
  return {
    api,
    commands,
    initialWorkspaceId: workspaceId,
    onSelectTask: vi.fn(),
    onWorkspaceChange: vi.fn(),
    pendingCommand: null,
    selectedTaskId: null,
  };
}

describe('TaskNavigation', () => {
  it.each(['submit_run', 'stop_run', 'respond'] as const)(
    'does not open Task creation while a %s command is pending',
    async (kind) => {
      const user = userEvent.setup();
      const { api, list } = setupApi();
      list.mockResolvedValue(taskPage([]));
      const commands = setupCommands();

      render(
        <TaskNavigation
          {...navigationProps(api, commands)}
          pendingCommand={pendingNonCreate(kind)}
        />,
      );

      await screen.findByText('No Tasks yet');
      const createButton = screen.getByRole('button', { name: 'New Task' });
      expect(createButton).toBeDisabled();
      await user.click(createButton);
      expect(screen.queryByRole('button', { name: 'Create Task' })).not.toBeInTheDocument();
      expect(commands.retryPendingCommand).not.toHaveBeenCalled();
      expect(commands.abandonConflictedCommand).not.toHaveBeenCalled();
    },
  );

  it('keeps an unknown create visible and non-cancellable across workspace changes', async () => {
    const user = userEvent.setup();
    const { api, list } = setupApi();
    api.workspaces.list = vi.fn<WebApi['workspaces']['list']>().mockResolvedValue({
      api_version: 1,
      items: [
        {
          availability: 'available',
          branch: 'main',
          is_default: true,
          label: 'Workspace A',
          workspace_id: workspaceId,
        },
        {
          availability: 'available',
          branch: 'main',
          is_default: false,
          label: 'Workspace B',
          workspace_id: otherWorkspaceId,
        },
      ],
      server_revision: 'rev_1',
    });
    list.mockResolvedValue(taskPage([]));
    const pendingCommand = pendingCreate('unknown');

    render(
      <TaskNavigation {...navigationProps(api, setupCommands())} pendingCommand={pendingCommand} />,
    );

    expect(await screen.findByRole('button', { name: 'Retry Task creation' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Cancel Task creation' })).toBeDisabled();
    await user.selectOptions(screen.getByRole('combobox', { name: 'Workspace' }), otherWorkspaceId);
    expect(screen.getByRole('button', { name: 'Retry Task creation' })).toBeVisible();
  });

  it('renders an explicit empty-workspace state instead of loading forever', async () => {
    const { api } = setupApi();
    api.workspaces.list = vi.fn<WebApi['workspaces']['list']>().mockResolvedValue({
      api_version: 1,
      items: [],
      server_revision: 'rev_1',
    });

    render(<TaskNavigation {...navigationProps(api, setupCommands())} initialWorkspaceId={null} />);

    expect(await screen.findByText('No workspaces available')).toBeVisible();
    expect(screen.queryByText('Loading Tasks')).not.toBeInTheDocument();
  });

  it('groups Tasks by Needs Attention, Running / Queued, and Recent', async () => {
    const { api, list } = setupApi();
    list.mockResolvedValue(
      taskPage([
        task('Needs review', 'needs_attention'),
        task('Active run', 'running'),
        task('Recent task', 'completed'),
      ]),
    );

    render(<TaskNavigation {...navigationProps(api, setupCommands())} />);

    expect(await screen.findByText('Needs Attention')).toBeVisible();
    expect(screen.getByText('Running / Queued')).toBeVisible();
    expect(screen.getByText('Recent')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Recent task' })).toBeVisible();
  });

  it('creates a Task in the selected workspace and selects its TaskId', async () => {
    const user = userEvent.setup();
    const { api, list } = setupApi();
    list.mockResolvedValue(taskPage([]));
    const commands = setupCommands();
    const nextTaskId = 'tsk_000000000000000000000002';
    commands.createTask.mockResolvedValue(created(nextTaskId));
    const props = navigationProps(api, commands);

    render(<TaskNavigation {...props} />);
    await screen.findByText('No Tasks yet');
    await user.click(screen.getByRole('button', { name: 'New Task' }));
    await user.click(screen.getByRole('button', { name: 'Create Task' }));

    expect(commands.createTask).toHaveBeenCalledWith(workspaceId);
    expect(props.onSelectTask).toHaveBeenCalledWith(nextTaskId);
  });

  it('keeps New Task open after an unknown outcome and selects only after retry acknowledgement', async () => {
    const user = userEvent.setup();
    const { api, list } = setupApi();
    list.mockResolvedValue(taskPage([]));
    const commands = setupCommands();
    const nextTaskId = 'tsk_000000000000000000000002';
    commands.createTask.mockRejectedValueOnce(new TypeError('offline'));
    commands.retryPendingCommand.mockResolvedValue(created(nextTaskId));
    const props = navigationProps(api, commands);

    render(<TaskNavigation {...props} />);
    await screen.findByText('No Tasks yet');
    await user.click(screen.getByRole('button', { name: 'New Task' }));
    await user.click(screen.getByRole('button', { name: 'Create Task' }));

    expect(await screen.findByRole('button', { name: 'Retry Task creation' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Abandon this creation' })).toBeVisible();
    expect(props.onSelectTask).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Retry Task creation' }));
    expect(commands.retryPendingCommand).toHaveBeenCalledTimes(1);
    expect(props.onSelectTask).toHaveBeenCalledWith(nextTaskId);
  });

  it('searches the complete workspace on the server instead of loaded pages', async () => {
    const user = userEvent.setup();
    const { api, list } = setupApi();
    list.mockImplementation((query) =>
      Promise.resolve(
        query.search === 'needle title'
          ? taskPage([task('Needle title in older page', 'completed')])
          : taskPage([task('Recent task', 'completed')]),
      ),
    );

    render(<TaskNavigation {...navigationProps(api, setupCommands())} />);
    await screen.findByRole('button', { name: 'Recent task' });
    await user.type(screen.getByRole('searchbox', { name: 'Search Tasks' }), 'needle title');

    expect(await screen.findByRole('button', { name: 'Needle title in older page' })).toBeVisible();
    expect(list).toHaveBeenCalledWith({
      workspace_id: workspaceId,
      search: 'needle title',
      limit: 100,
      cursor: null,
    });
  });

  it('loads the next server-ordered page only after explicit navigation', async () => {
    const user = userEvent.setup();
    const { api, list } = setupApi();
    list
      .mockResolvedValueOnce(taskPage([task('First page', 'completed')], 'page:next'))
      .mockResolvedValueOnce(taskPage([task('Older page', 'completed')], null));

    render(<TaskNavigation {...navigationProps(api, setupCommands())} />);
    await screen.findByRole('button', { name: 'First page' });
    expect(list).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: 'Load more Tasks' }));

    expect(await screen.findByRole('button', { name: 'Older page' })).toBeVisible();
    expect(list).toHaveBeenLastCalledWith({
      workspace_id: workspaceId,
      search: null,
      limit: 100,
      cursor: 'page:next',
    });
  });

  it('ignores a stale load-more failure after switching workspace', async () => {
    const user = userEvent.setup();
    const { api, list } = setupApi();
    api.workspaces.list = vi.fn<WebApi['workspaces']['list']>().mockResolvedValue({
      api_version: 1,
      items: [
        {
          availability: 'available',
          branch: 'main',
          is_default: true,
          label: 'Workspace A',
          workspace_id: workspaceId,
        },
        {
          availability: 'available',
          branch: 'main',
          is_default: false,
          label: 'Workspace B',
          workspace_id: otherWorkspaceId,
        },
      ],
      server_revision: 'rev_1',
    });
    const oldPage = deferredPage();
    list.mockImplementation((query) => {
      if (query.cursor === 'page:next') return oldPage.promise;
      if (query.workspace_id === otherWorkspaceId) {
        return Promise.resolve(taskPage([taskForWorkspace('Task B', otherWorkspaceId)]));
      }
      return Promise.resolve(taskPage([task('Task A', 'completed')], 'page:next'));
    });

    render(<TaskNavigation {...navigationProps(api, setupCommands())} />);
    await screen.findByRole('button', { name: 'Task A' });
    await user.click(screen.getByRole('button', { name: 'Load more Tasks' }));
    await user.selectOptions(screen.getByRole('combobox', { name: 'Workspace' }), otherWorkspaceId);
    expect(await screen.findByRole('button', { name: 'Task B' })).toBeVisible();
    oldPage.reject(new TypeError('stale A failure'));
    await new Promise((resolve) => window.setTimeout(resolve, 0));

    expect(screen.getByRole('button', { name: 'Task B' })).toBeVisible();
    expect(screen.queryByText('Tasks unavailable')).not.toBeInTheDocument();
  });

  it('does not select a created Task when the store reports a stale acknowledgement', async () => {
    const user = userEvent.setup();
    const { api, list } = setupApi();
    list.mockResolvedValue(taskPage([]));
    const commands = setupCommands();
    commands.createTask.mockResolvedValue(undefined);
    const props = navigationProps(api, commands);

    render(<TaskNavigation {...props} />);
    await screen.findByText('No Tasks yet');
    await user.click(screen.getByRole('button', { name: 'New Task' }));
    await user.click(screen.getByRole('button', { name: 'Create Task' }));

    expect(await screen.findByRole('searchbox', { name: 'Search Tasks' })).toBeVisible();
    expect(props.onSelectTask).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Retry Task creation' })).not.toBeInTheDocument();
  });

  it('closes a conflicted creation to review the reconciled Task list', async () => {
    const user = userEvent.setup();
    const { api, list } = setupApi();
    list.mockResolvedValue(taskPage([task('Reconciled task', 'completed')]));
    const pendingCommand = pendingCreate('conflicted');

    render(
      <TaskNavigation {...navigationProps(api, setupCommands())} pendingCommand={pendingCommand} />,
    );
    await user.click(screen.getByRole('button', { name: 'Review Tasks' }));

    expect(await screen.findByRole('button', { name: 'Reconciled task' })).toBeVisible();
    expect(screen.queryByText('Task creation could not be reconciled.')).not.toBeInTheDocument();
    expect(list).toHaveBeenCalled();
  });
});

describe('NewTaskForm', () => {
  it('shows a reconciliation prompt after create idempotency conflict', () => {
    const pending = pendingCreate('conflicted');

    render(
      <NewTaskForm
        workspaceId={workspaceId}
        pending={pending}
        createTask={vi.fn()}
        retryPendingCommand={vi.fn()}
        abandonConflictedCommand={vi.fn()}
        onCreated={vi.fn()}
        onCancel={vi.fn()}
        onReviewTasks={vi.fn()}
      />,
    );

    expect(screen.getByRole('alert')).toHaveTextContent('Task creation could not be reconciled');
    expect(screen.getByRole('button', { name: 'Review Tasks' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Abandon this creation' })).toBeVisible();
  });
});

function pendingCreate(
  status: 'unknown' | 'conflicted' | 'failed',
): Extract<PendingCommand, { kind: 'create_task' }> {
  return {
    commandId: 1,
    controller: new AbortController(),
    draftGeneration: 0,
    kind: 'create_task',
    status,
    taskGeneration: 0,
    taskId: null,
    body: { workspace_id: workspaceId, idempotency_key: 'idem-pending' },
  };
}

function pendingNonCreate(
  kind: 'submit_run' | 'stop_run' | 'respond',
): Exclude<PendingCommand, { kind: 'create_task' }> {
  const meta = {
    commandId: 2,
    controller: new AbortController(),
    draftGeneration: 0,
    status: 'unknown' as const,
    taskGeneration: 0,
    taskId,
  };
  switch (kind) {
    case 'submit_run':
      return {
        ...meta,
        kind,
        body: {
          expected_task_revision: 4,
          idempotency_key: 'idem-submit',
          message: 'pending submit',
          skill_ids: [],
          tier_id: 'tier:balanced',
        },
      };
    case 'stop_run':
      return {
        ...meta,
        kind,
        runId: 'run_000000000000000000000001',
        body: { expected_task_revision: 4, idempotency_key: 'idem-stop' },
      };
    case 'respond':
      return {
        ...meta,
        kind,
        interactionId: 'int_000000000000000000000001',
        body: {
          choice_id: 'choice-1',
          expected_task_revision: 4,
          idempotency_key: 'idem-respond',
        },
      };
  }
}

function taskForWorkspace(title: string, id: string): TaskSummary {
  return { ...task(title, 'completed'), workspace_id: id };
}

function deferredPage() {
  let resolve: (page: TaskPage) => void = () => undefined;
  let reject: (reason: unknown) => void = () => undefined;
  const promise = new Promise<TaskPage>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}
