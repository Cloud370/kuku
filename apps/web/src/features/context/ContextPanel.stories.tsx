import type { Meta, StoryObj } from '@storybook/react';
import { useState } from 'react';
import { expect, within } from 'storybook/test';

import type { ContextSnapshot } from '../../api/generated';
import { ContextPanel, type ContextSectionKey } from './ContextPanel';
import { defaultOpenSections } from './contextSelectors';
import {
  catalogFixture,
  contextFixture,
  apiErrorFixture,
  taskId,
  workspaceId,
} from './testFixtures';

interface StoryHarnessProps {
  error?: Error;
  height?: string;
  pending?: boolean;
  snapshot?: ContextSnapshot;
  staged?: string[];
  width?: string;
}

function StoryHarness({
  error,
  height = '42rem',
  pending = false,
  snapshot = contextFixture(),
  staged = [],
  width = '24rem',
}: StoryHarnessProps) {
  const [openSections, setOpenSections] = useState<ContextSectionKey[]>(
    defaultOpenSections(snapshot, staged),
  );
  const [stagedSkillIds, setStagedSkillIds] = useState(staged);
  const [selectedRequestId, setSelectedRequestId] = useState<string | null>(null);
  globalThis.fetch = pending
    ? () => new Promise<Response>(() => undefined)
    : (input: RequestInfo | URL) => {
        const path =
          typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
        if (path.includes('/catalog')) {
          return Promise.resolve(Response.json(catalogFixture()));
        }
        if (error !== undefined) {
          return Promise.resolve(Response.json(apiErrorFixture(), { status: 404 }));
        }
        return Promise.resolve(Response.json(snapshot));
      };

  return (
    <div style={{ height, width, maxWidth: '100%' }}>
      <ContextPanel
        onOpenAgent={() => undefined}
        onOpenFile={() => undefined}
        onOpenSectionsChange={setOpenSections}
        onSelectRequest={setSelectedRequestId}
        onStageSkill={(skillId) => {
          setStagedSkillIds((current) => [...current, skillId]);
        }}
        onUnstageSkill={(skillId) => {
          setStagedSkillIds((current) => current.filter((value) => value !== skillId));
        }}
        openSections={openSections}
        selectedRequestId={selectedRequestId}
        stagedSkillIds={stagedSkillIds}
        taskId={taskId}
        workspaceId={workspaceId}
      />
    </div>
  );
}

const meta: Meta<typeof ContextPanel> = {
  title: 'Experience/Context',
  component: ContextPanel,
};

export default meta;
type Story = StoryObj<typeof ContextPanel>;

const assertStickySummary: NonNullable<Story['play']> = async ({ canvasElement }) => {
  const canvas = within(canvasElement);
  const summaryLabel = await canvas.findByText('Current Context');
  const summary = summaryLabel.parentElement?.parentElement;
  const accordion = canvas.getByLabelText('Context sections');
  if (summary === null || summary === undefined) {
    throw new globalThis.Error('Context summary is missing');
  }
  const summaryTop = summary.getBoundingClientRect().top;
  const bodyScroll = document.scrollingElement?.scrollTop ?? 0;
  accordion.scrollTop = 80;
  accordion.dispatchEvent(new Event('scroll'));
  await new Promise<void>((resolve) => {
    requestAnimationFrame(() => {
      resolve();
    });
  });

  await expect(accordion.scrollTop).toBeGreaterThan(0);
  await expect(Math.abs(summary.getBoundingClientRect().top - summaryTop)).toBeLessThanOrEqual(1);
  await expect(accordion.getBoundingClientRect().top).toBeGreaterThanOrEqual(
    summary.getBoundingClientRect().bottom - 1,
  );
  await expect(document.scrollingElement?.scrollTop ?? 0).toBe(bodyScroll);
};

export const Loading: Story = { render: () => <StoryHarness pending /> };

export const EmptyWithoutStaged: Story = {
  render: () => <StoryHarness snapshot={contextFixture({ selected_request: null })} />,
};

export const Error: Story = {
  render: () => (
    <StoryHarness
      error={Object.assign(new globalThis.Error('Request not found'), {
        code: 'request_not_found',
      })}
    />
  ),
};

export const Verified: Story = { render: () => <StoryHarness /> };

export const VerifiedWithWarnings: Story = {
  render: () => <StoryHarness snapshot={contextFixture()} />,
};

export const PreviewWithStagedSkill: Story = {
  render: () => <StoryHarness staged={['skill:project:docs']} />,
};

export const StickySummaryDesktop: Story = {
  render: () => <StoryHarness height="34rem" width="28rem" />,
  play: assertStickySummary,
};

export const StickySummary360: Story = {
  render: () => <StoryHarness height="40rem" width="360px" />,
  play: assertStickySummary,
};

export const LongPath: Story = {
  render: () => {
    const snapshot = contextFixture();
    const observation = snapshot.sections.observations.at(0);
    if (observation !== undefined) {
      observation.relative_path = `${'deeply-nested/'.repeat(12)}implementation-with-a-long-name.rs`;
    }
    return <StoryHarness snapshot={snapshot} width="360px" />;
  },
};

export const LongSkillName: Story = {
  render: () => <StoryHarness staged={['skill:project:docs']} width="360px" />,
};
