import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { SkillCatalogEntry } from '../../api/generated';
import { SkillPicker } from './SkillPicker';

const entries: SkillCatalogEntry[] = [
  {
    description: 'Review Rust changes',
    name: 'rust-review',
    skill_id: 'skill:project:rust-review',
    source: { id: 'src-rust', relative_path: null, scope: 'project' },
  },
  {
    description: 'Summarize documentation',
    name: 'docs-summary',
    skill_id: 'skill:workspace:docs-summary',
    source: { id: 'src-docs', relative_path: null, scope: 'workspace' },
  },
];

afterEach(() => {
  cleanup();
});

describe('SkillPicker', () => {
  it('filters canonical entries and stages the selected Skill ID', async () => {
    const user = userEvent.setup();
    const onSearch = vi.fn();
    const onStage = vi.fn();
    render(<SkillPicker entries={entries} onSearch={onSearch} onStage={onStage} />);

    await user.type(screen.getByRole('searchbox', { name: 'Search Skills' }), 'rust');

    expect(onSearch).toHaveBeenLastCalledWith('rust');
    expect(screen.getByRole('option', { name: 'rust-review' })).toBeVisible();
    expect(screen.queryByRole('option', { name: 'docs-summary' })).toBeNull();
    await user.click(screen.getByRole('option', { name: 'rust-review' }));
    expect(onStage).toHaveBeenCalledWith(entries[0]);
  });
});
