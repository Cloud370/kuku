import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ComponentProps } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { SkillCatalogEntry, TierCatalogEntry } from '../../api/generated';
import type { LocalDraft } from '../state';
import type { PendingCommand } from '../workbenchStore';
import { Composer } from './Composer';

const tiers: TierCatalogEntry[] = [
  {
    tier: {
      is_default: true,
      label: 'Balanced',
      model: 'fixture-model',
      provider: 'fixture-provider',
      purpose: 'General work',
      think: null,
      tier_id: 'tier:balanced',
    },
  },
  {
    tier: {
      is_default: false,
      label: 'Fast',
      model: 'fixture-fast',
      provider: 'fixture-provider',
      purpose: 'Quick fixes',
      think: null,
      tier_id: 'tier:fast',
    },
  },
];

const skills: SkillCatalogEntry[] = [
  {
    description: 'Review Rust changes',
    name: 'rust-review',
    skill_id: 'skill:project:rust-review',
    source: { id: 'src-rust', relative_path: null, scope: 'project' },
  },
];

function draft(overrides: Partial<LocalDraft> = {}): LocalDraft {
  return { skillIds: [], text: '', tierId: 'tier:balanced', ...overrides };
}

function props(overrides: Partial<ComponentProps<typeof Composer>> = {}) {
  return {
    activeRunId: null,
    catalogReady: true,
    defaultTierId: 'tier:balanced',
    draft: draft(),
    loadedSkillCount: 0,
    onDraftChange: vi.fn(),
    onOpenLoadedSkills: vi.fn(),
    onRetryPendingCommand: vi.fn(),
    onStop: vi.fn(),
    onSubmit: vi.fn().mockResolvedValue(undefined),
    pendingCommand: null as PendingCommand | null,
    skills,
    taskId: 'tsk_000000000000000000000001',
    tiers,
    ...overrides,
  };
}

afterEach(() => {
  cleanup();
});

describe('Composer', () => {
  it('uses configured Tier names and purposes, including custom Tiers', async () => {
    const user = userEvent.setup();
    render(<Composer {...props()} />);

    await user.click(screen.getByRole('button', { name: 'Choose Tier' }));

    expect(screen.getByRole('option', { name: /Fast/ })).toBeVisible();
    expect(screen.getByText('Quick fixes')).toBeVisible();
  });

  it('submits message, Tier, and staged Skill IDs atomically and clears on success', async () => {
    const user = userEvent.setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const onDraftChange = vi.fn();
    const view = render(<Composer {...props({ onDraftChange, onSubmit })} />);

    await user.type(screen.getByRole('textbox', { name: 'Message' }), 'inspect');
    await user.click(screen.getByRole('button', { name: 'Add Skill' }));
    await user.click(screen.getByRole('option', { name: 'rust-review' }));
    view.rerender(
      <Composer
        {...props({
          draft: draft({ skillIds: ['skill:project:rust-review'], text: 'inspect' }),
          onDraftChange,
          onSubmit,
        })}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Send' }));

    expect(onSubmit).toHaveBeenCalledWith({
      message: 'inspect',
      skill_ids: ['skill:project:rust-review'],
      tier_id: 'tier:balanced',
    });
    expect(onDraftChange).toHaveBeenLastCalledWith(draft());
  });

  it('uses one catalog entry for Skill picker selection and read-only preview', async () => {
    const user = userEvent.setup();
    render(<Composer {...props()} />);

    await user.click(screen.getByRole('button', { name: 'Add Skill' }));
    await user.click(screen.getByRole('option', { name: 'rust-review' }));

    expect(screen.getByRole('region', { name: 'rust-review preview' })).toHaveTextContent(
      'Review Rust changes',
    );
  });

  it('opens loaded Skill inventory and offers retry without clearing an unknown draft', async () => {
    const user = userEvent.setup();
    const onOpenLoadedSkills = vi.fn();
    const onRetryPendingCommand = vi.fn();
    const pending = {
      body: {
        expected_task_revision: 3,
        idempotency_key: 'idem-fixed',
        message: 'inspect',
        skill_ids: [],
        tier_id: 'tier:balanced',
      },
      commandId: 1,
      controller: new AbortController(),
      draftGeneration: 1,
      kind: 'submit_run',
      status: 'unknown',
      taskGeneration: 1,
      taskId: 'tsk_000000000000000000000001',
    } satisfies PendingCommand;
    render(
      <Composer
        {...props({
          draft: draft({ text: 'inspect' }),
          loadedSkillCount: 8,
          onOpenLoadedSkills,
          onRetryPendingCommand,
          pendingCommand: pending,
        })}
      />,
    );

    await user.click(screen.getByRole('button', { name: '8 Skills loaded' }));
    await user.click(screen.getByRole('button', { name: 'Retry send' }));

    expect(onOpenLoadedSkills).toHaveBeenCalledTimes(1);
    expect(onRetryPendingCommand).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('inspect');
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  });

  it('disables submission until a Task is selected', () => {
    render(<Composer {...props({ draft: draft({ text: 'inspect' }), taskId: null })} />);

    expect(screen.getByRole('textbox', { name: 'Message' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  });

  it('does not clear a new Task draft when an earlier submission resolves', async () => {
    const user = userEvent.setup();
    let resolveSubmit: (() => void) | undefined;
    const onSubmit = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveSubmit = resolve;
        }),
    );
    const onDraftChange = vi.fn();
    const view = render(
      <Composer {...props({ draft: draft({ text: 'first' }), onDraftChange, onSubmit })} />,
    );

    await user.click(screen.getByRole('button', { name: 'Send' }));
    view.rerender(
      <Composer
        {...props({
          draft: draft({ text: 'second' }),
          onDraftChange,
          onSubmit,
          taskId: 'tsk_000000000000000000000002',
        })}
      />,
    );
    resolveSubmit?.();
    await Promise.resolve();

    expect(onDraftChange).not.toHaveBeenCalled();
  });
});
