import { Plus, Send } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';

import type { SkillCatalogEntry, TierCatalogEntry } from '../../api/generated';
import type { LocalDraft } from '../state';
import type { PendingCommand } from '../workbenchStore';
import { PendingCommandNotice } from './PendingCommandNotice';
import { SkillPicker } from './SkillPicker';
import { SkillPreview } from './SkillPreview';
import { SkillStageStrip } from './SkillStageStrip';
import { StopButton } from './StopButton';
import { TierSelector } from './TierSelector';

export interface ComposerSubmitInput {
  message: string;
  skill_ids: string[];
  tier_id: string;
}

interface ComposerProps {
  activeRunId: string | null;
  catalogReady: boolean;
  defaultTierId: string;
  draft: LocalDraft;
  loadedSkillCount: number;
  onDraftChange: (draft: LocalDraft) => void;
  onOpenLoadedSkills: () => void;
  onRetryPendingCommand: () => void;
  onSearchSkills?: (query: string) => void;
  onStop: () => void;
  onSubmit: (input: ComposerSubmitInput) => Promise<void> | void;
  pendingCommand: PendingCommand | null;
  skills: SkillCatalogEntry[];
  taskId: string | null;
  tiers: TierCatalogEntry[];
}

export function Composer({
  activeRunId,
  catalogReady,
  defaultTierId,
  draft,
  loadedSkillCount,
  onDraftChange,
  onOpenLoadedSkills,
  onRetryPendingCommand,
  onSearchSkills,
  onStop,
  onSubmit,
  pendingCommand,
  skills,
  taskId,
  tiers,
}: ComposerProps) {
  const [pickerOpen, setPickerOpen] = useState(false);
  const [preview, setPreview] = useState<SkillCatalogEntry | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const currentDraft = useRef(draft);
  const currentTaskId = useRef(taskId);
  useEffect(() => {
    currentDraft.current = draft;
    currentTaskId.current = taskId;
  }, [draft, taskId]);
  const stagedEntries = useMemo(
    () =>
      draft.skillIds
        .map((id) => skills.find((entry) => entry.skill_id === id))
        .filter((entry): entry is SkillCatalogEntry => entry !== undefined),
    [draft.skillIds, skills],
  );
  const hasActiveRun = activeRunId !== null;
  const selectedTierId = resolveTierId(draft.tierId, defaultTierId, tiers);
  const disabled =
    submitting ||
    pendingCommand !== null ||
    taskId === null ||
    !catalogReady ||
    selectedTierId === null;

  const submit = async () => {
    const message = draft.text.trim();
    if (message.length === 0 || disabled || hasActiveRun) return;
    const submittedDraft = draft;
    const submittedTaskId = taskId;
    setSubmitting(true);
    try {
      await onSubmit({ message, skill_ids: draft.skillIds, tier_id: selectedTierId });
      if (
        currentTaskId.current === submittedTaskId &&
        draftsEqual(currentDraft.current, submittedDraft)
      ) {
        onDraftChange({ ...submittedDraft, skillIds: [], text: '' });
      }
    } finally {
      setSubmitting(false);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      void submit();
    }
  };

  return (
    <section
      aria-label="Composer"
      className="border-t border-[var(--color-border)] bg-[var(--color-surface)] p-3"
    >
      {pendingCommand?.status === 'unknown' ? (
        <PendingCommandNotice command={pendingCommand} onRetry={onRetryPendingCommand} />
      ) : null}
      <div className="mx-auto max-w-4xl">
        <textarea
          aria-label="Message"
          className="min-h-20 w-full resize-y border border-[var(--color-border)] bg-[var(--color-surface-raised)] p-3 text-sm outline-none focus-visible:border-[var(--color-accent)]"
          disabled={disabled || hasActiveRun}
          onChange={(event) => {
            onDraftChange({ ...draft, text: event.target.value });
          }}
          onKeyDown={onKeyDown}
          placeholder="Message the Agent"
          value={draft.text}
        />
        {pickerOpen ? (
          <SkillPicker
            entries={skills}
            onStage={(entry) => {
              if (!draft.skillIds.includes(entry.skill_id)) {
                onDraftChange({ ...draft, skillIds: [...draft.skillIds, entry.skill_id] });
              }
              setPreview(entry);
              setPickerOpen(false);
              onSearchSkills?.('');
            }}
            onSearch={onSearchSkills}
          />
        ) : null}
        {preview !== null ? <SkillPreview entry={preview} /> : null}
        <div className="mt-2 flex min-w-0 items-center gap-2">
          <button
            aria-label="Add Skill"
            className="inline-flex size-8 shrink-0 items-center justify-center border border-[var(--color-border)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
            disabled={disabled || hasActiveRun}
            onClick={() => {
              if (pickerOpen) onSearchSkills?.('');
              setPickerOpen((current) => !current);
            }}
            title="Add Skill"
            type="button"
          >
            <Plus aria-hidden="true" size={15} />
          </button>
          <SkillStageStrip
            loadedSkillCount={loadedSkillCount}
            onOpenLoadedSkills={onOpenLoadedSkills}
            onRemove={(skillId) => {
              onDraftChange({ ...draft, skillIds: draft.skillIds.filter((id) => id !== skillId) });
            }}
            stagedEntries={stagedEntries}
          />
          <div className="ml-auto flex shrink-0 items-center gap-2">
            <TierSelector
              onChange={(tierId) => {
                onDraftChange({ ...draft, tierId });
              }}
              selectedTierId={selectedTierId}
              tiers={tiers}
            />
            {hasActiveRun ? (
              <StopButton disabled={pendingCommand !== null} onStop={onStop} />
            ) : null}
            <button
              aria-label="Send"
              className="inline-flex size-9 shrink-0 items-center justify-center bg-[var(--color-accent)] text-[var(--color-accent-contrast)] hover:opacity-90 focus-visible:outline-2 focus-visible:outline-[var(--color-accent)] disabled:opacity-40"
              disabled={disabled || hasActiveRun || draft.text.trim().length === 0}
              onClick={() => {
                void submit();
              }}
              title="Send"
              type="button"
            >
              <Send aria-hidden="true" size={15} />
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}

function resolveTierId(
  draftTierId: string | null,
  defaultTierId: string,
  tiers: TierCatalogEntry[],
): string | null {
  const hasTier = (tierId: string | null): tierId is string =>
    tierId !== null && tiers.some(({ tier }) => tier.tier_id === tierId);
  if (hasTier(draftTierId)) return draftTierId;
  if (hasTier(defaultTierId)) return defaultTierId;
  return tiers.find(({ tier }) => tier.is_default)?.tier.tier_id ?? tiers[0]?.tier.tier_id ?? null;
}

function draftsEqual(left: LocalDraft, right: LocalDraft): boolean {
  return (
    left.text === right.text &&
    left.tierId === right.tierId &&
    left.skillIds.length === right.skillIds.length &&
    left.skillIds.every((id, index) => id === right.skillIds[index])
  );
}
