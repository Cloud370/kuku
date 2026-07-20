import { Bot, ChevronDown, ChevronUp, FileText, LoaderCircle, Wrench } from 'lucide-react';
import { useState } from 'react';

import type { ActivityProjection } from '../../api/generated';

interface ActivityItemProps {
  activity: ActivityProjection;
  onOpenFile: (workspaceId: string, relativePath: string) => void;
}

export function ActivityItem({ activity, onOpenFile }: ActivityItemProps) {
  const [expanded, setExpanded] = useState(false);
  const delegated = activity.kind === 'delegated_agent';
  const detailLabel = activity.kind === 'tool' ? 'Tool details' : 'Activity details';

  return (
    <article className="border-l-2 border-[var(--color-border)] py-2 pl-3 text-sm">
      <div className="flex min-w-0 items-center gap-2">
        {activity.status === 'running' ? (
          <LoaderCircle aria-hidden="true" className="shrink-0 animate-spin" size={15} />
        ) : delegated ? (
          <Bot aria-hidden="true" className="shrink-0" size={15} />
        ) : (
          <Wrench aria-hidden="true" className="shrink-0" size={15} />
        )}
        <span className="min-w-0 flex-1 truncate font-medium">
          {delegated ? 'Delegated Agent' : activity.title}
        </span>
        {activity.detail !== null ? (
          <button
            aria-expanded={expanded}
            aria-label={
              expanded ? `Hide ${detailLabel.toLowerCase()}` : `Show ${detailLabel.toLowerCase()}`
            }
            className="inline-flex size-8 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
            onClick={() => {
              setExpanded((current) => !current);
            }}
            title={
              expanded ? `Hide ${detailLabel.toLowerCase()}` : `Show ${detailLabel.toLowerCase()}`
            }
            type="button"
          >
            {expanded ? (
              <ChevronUp aria-hidden="true" size={15} />
            ) : (
              <ChevronDown aria-hidden="true" size={15} />
            )}
          </button>
        ) : null}
      </div>
      {expanded && activity.detail !== null ? (
        <div
          aria-label={detailLabel}
          className="mt-2 whitespace-pre-wrap rounded-[var(--radius-sm)] bg-[var(--color-surface-raised)] p-3 font-mono text-xs"
          role="region"
        >
          {activity.detail}
        </div>
      ) : null}
      {activity.file_references.length > 0 ? (
        <div className="mt-2 flex flex-wrap gap-2">
          {activity.file_references.map((reference) => (
            <button
              aria-label={`Open ${reference.label}`}
              className="inline-flex items-center gap-1 rounded-[var(--radius-sm)] border border-[var(--color-border)] px-2 py-1 text-xs hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              key={`${reference.workspace_id}:${reference.relative_path}`}
              onClick={() => {
                onOpenFile(reference.workspace_id, reference.relative_path);
              }}
              title={`Open ${reference.label}`}
              type="button"
            >
              <FileText aria-hidden="true" size={13} />
              {reference.label}
            </button>
          ))}
        </div>
      ) : null}
    </article>
  );
}
