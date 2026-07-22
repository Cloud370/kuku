import { Bot, FileText, Plus, X } from 'lucide-react';
import type { ReactNode } from 'react';

import type { SkillLoadOrigin, WorkspaceId } from '../../api/generated';
import { SafeMarkdown } from '../../components/content/SafeMarkdown';
import type { ContextSectionKey } from './contextSections';
import type { ContextViewModel } from './contextSelectors';
import { isWorkspaceRelativePath } from './contextSelectors';

interface ContextSectionContentProps {
  section: ContextSectionKey;
  view: ContextViewModel;
  workspaceId: WorkspaceId;
  onOpenAgent: (conversationId: string) => void;
  onOpenFile: (workspaceId: WorkspaceId, relativePath: string) => void;
  onStageSkill: (skillId: string) => void;
  onUnstageSkill: (skillId: string) => void;
}

function Empty({ children }: { children: ReactNode }) {
  return <p className="py-2 text-xs text-[var(--color-text-muted)]">{children}</p>;
}

function SourceLine({ id, path, scope }: { id: string; path: string | null; scope: string }) {
  return (
    <p className="mt-1 break-all font-mono text-xs text-[var(--color-text-muted)]">
      {scope} · {path ?? id}
    </p>
  );
}

function formatMetric(value: number | null, suffix = ''): string | null {
  return value === null ? null : `${value.toLocaleString('en-US')}${suffix}`;
}

const SKILL_ORIGIN_LABELS: Record<SkillLoadOrigin, string> = {
  agent: 'Loaded by Agent',
  bootstrap: 'Loaded by Bootstrap',
  project: 'Loaded by Project',
  you: 'Loaded by You',
};

function UsageScope({
  label,
  usage,
}: {
  label: string;
  usage: ContextViewModel['usage']['thisTask'];
}) {
  const metrics = [
    ['Input', formatMetric(usage.input_tokens)],
    ['Output', formatMetric(usage.output_tokens)],
    ['Cached input', formatMetric(usage.cached_input_tokens)],
    ['Cache creation', formatMetric(usage.cache_creation_input_tokens)],
    ['Elapsed', formatMetric(usage.elapsed_ms, ' ms')],
    [
      'Cost',
      usage.cost === null
        ? null
        : `${usage.cost.currency} ${(usage.cost.micros / 1_000_000).toFixed(6)}`,
    ],
  ].filter((metric): metric is [string, string] => metric[1] !== null);
  return (
    <div>
      <h4 className="text-xs font-semibold">{label}</h4>
      <dl className="mt-2 grid grid-cols-2 gap-x-3 gap-y-2 text-xs">
        <div>
          <dt className="text-[var(--color-text-muted)]">Requests</dt>
          <dd className="tabular-nums">{usage.request_count}</dd>
        </div>
        {metrics.map(([name, value]) => (
          <div key={name}>
            <dt className="text-[var(--color-text-muted)]">{name}</dt>
            <dd className="tabular-nums">{value}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

export function ContextSectionContent({
  section,
  view,
  workspaceId,
  onOpenAgent,
  onOpenFile,
  onStageSkill,
  onUnstageSkill,
}: ContextSectionContentProps) {
  if (section === 'staged') {
    return view.staged.length === 0 ? null : (
      <ul className="divide-y divide-[var(--color-border)]">
        {view.staged.map((skill) => (
          <li className="flex min-w-0 items-center gap-2 py-2" key={skill.skill_id}>
            <span className="min-w-0 flex-1 break-words text-sm">{skill.name}</span>
            <button
              aria-label={`Remove ${skill.name}`}
              className="inline-flex size-8 shrink-0 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              onClick={() => {
                onUnstageSkill(skill.skill_id);
              }}
              title={`Remove ${skill.name}`}
              type="button"
            >
              <X aria-hidden="true" size={15} />
            </button>
          </li>
        ))}
      </ul>
    );
  }
  if (section === 'skills') {
    return view.sections.skills.length === 0 ? (
      <Empty>No Skills were loaded for this Request.</Empty>
    ) : (
      <ul className="divide-y divide-[var(--color-border)]">
        {view.sections.skills.map((skill) => (
          <li className="py-2" key={skill.skill_id}>
            <div className="flex items-center justify-between gap-2">
              <span className="min-w-0 break-words text-sm font-medium">{skill.name}</span>
              <span className="shrink-0 text-xs text-[var(--color-text-muted)]">
                {SKILL_ORIGIN_LABELS[skill.origin]}
              </span>
            </div>
            <SafeMarkdown source={skill.description} />
            <SourceLine
              id={skill.source.id}
              path={skill.source.relative_path}
              scope={skill.source.scope}
            />
          </li>
        ))}
      </ul>
    );
  }
  if (section === 'instructions' || section === 'memory') {
    const entries = view.sections[section];
    return entries.length === 0 ? (
      <Empty>No {section} sources were included.</Empty>
    ) : (
      <ul className="divide-y divide-[var(--color-border)]">
        {entries.map((entry) => (
          <li className="py-2" key={`${entry.source.id}:${entry.content_hash}`}>
            <p className="text-sm font-medium">{entry.label}</p>
            <SourceLine
              id={entry.source.id}
              path={entry.source.relative_path}
              scope={entry.source.scope}
            />
            <p className="mt-1 break-all font-mono text-xs text-[var(--color-text-muted)]">
              {entry.content_hash}
            </p>
          </li>
        ))}
      </ul>
    );
  }
  if (section === 'conversation') {
    const conversation = view.sections.conversation;
    return (
      <dl className="grid grid-cols-2 gap-3 py-2 text-xs">
        <div>
          <dt className="text-[var(--color-text-muted)]">Retained turns</dt>
          <dd>{conversation.retained_turns}</dd>
        </div>
        <div>
          <dt className="text-[var(--color-text-muted)]">Handoffs</dt>
          <dd>{conversation.handoff_boundaries}</dd>
        </div>
        <div>
          <dt className="text-[var(--color-text-muted)]">Summarized</dt>
          <dd>{conversation.history_summarized ? 'Yes' : 'No'}</dd>
        </div>
        <div>
          <dt className="text-[var(--color-text-muted)]">Delegated results</dt>
          <dd>{conversation.delegated_results.length}</dd>
        </div>
      </dl>
    );
  }
  if (section === 'observations') {
    return view.sections.observations.length === 0 ? (
      <Empty>No workspace observations were recorded.</Empty>
    ) : (
      <ul className="divide-y divide-[var(--color-border)]">
        {view.sections.observations.map((observation) => {
          const path = observation.relative_path;
          const clickable = path !== null && isWorkspaceRelativePath(path);
          return (
            <li className="py-2" key={`${observation.request_id}:${observation.tool_call_id}`}>
              <div className="flex min-w-0 items-start gap-2">
                <FileText aria-hidden="true" className="mt-0.5 shrink-0" size={14} />
                <div className="min-w-0 flex-1">
                  {path === null ? (
                    <span className="text-xs text-[var(--color-text-muted)]">No file path</span>
                  ) : clickable ? (
                    <button
                      aria-label={`Open ${path}`}
                      className="max-w-full break-all text-left font-mono text-xs text-[var(--color-accent)] hover:underline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
                      onClick={() => {
                        onOpenFile(workspaceId, path);
                      }}
                      type="button"
                    >
                      {path}
                    </button>
                  ) : (
                    <span
                      aria-disabled="true"
                      className="break-all font-mono text-xs text-[var(--color-text-muted)]"
                    >
                      {path}
                    </span>
                  )}
                  <p className="mt-1 text-xs text-[var(--color-text-secondary)]">
                    {observation.summary}
                  </p>
                  <p className="mt-1 text-xs text-[var(--color-text-muted)]">
                    {observation.retention} · {observation.current_drift}
                  </p>
                </div>
              </div>
            </li>
          );
        })}
      </ul>
    );
  }
  if (section === 'agents') {
    return view.sections.agents.length === 0 ? (
      <Empty>No delegated Agents contributed to this Context.</Empty>
    ) : (
      <ul className="divide-y divide-[var(--color-border)]">
        {view.sections.agents.map((agent) => (
          <li className="py-2" key={agent.conversation_id}>
            <button
              aria-label={`Open ${agent.agent.name}`}
              className="flex w-full min-w-0 items-center gap-2 rounded-[var(--radius-sm)] p-1 text-left hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
              onClick={() => {
                onOpenAgent(agent.conversation_id);
              }}
              type="button"
            >
              <Bot aria-hidden="true" className="shrink-0" size={15} />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium">{agent.agent.name}</span>
                <span className="block truncate text-xs text-[var(--color-text-muted)]">
                  {agent.tier.label} · {agent.status}
                </span>
              </span>
            </button>
          </li>
        ))}
      </ul>
    );
  }
  if (section === 'discoverable') {
    return view.discoverable.skills.length === 0 ? (
      <Empty>No discoverable Skills are available.</Empty>
    ) : (
      <ul className="divide-y divide-[var(--color-border)]">
        {view.discoverable.skills.map((skill) => {
          const loaded = view.sections.skills.some((entry) => entry.skill_id === skill.skill_id);
          const staged = view.staged.some((entry) => entry.skill_id === skill.skill_id);
          return (
            <li className="flex min-w-0 items-start gap-2 py-2" key={skill.skill_id}>
              <div className="min-w-0 flex-1">
                <p className="break-words text-sm font-medium">{skill.name}</p>
                <SafeMarkdown source={skill.description} />
                <p className="text-xs text-[var(--color-text-muted)]">{skill.truth}</p>
              </div>
              <button
                aria-label={`Stage ${skill.name}`}
                className="inline-flex size-8 shrink-0 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)] disabled:opacity-40"
                disabled={loaded || staged}
                onClick={() => {
                  onStageSkill(skill.skill_id);
                }}
                title={
                  loaded ? 'Already loaded' : staged ? 'Already staged' : `Stage ${skill.name}`
                }
                type="button"
              >
                <Plus aria-hidden="true" size={15} />
              </button>
            </li>
          );
        })}
      </ul>
    );
  }
  if (section === 'capabilities') {
    return view.sections.capabilities.length === 0 ? (
      <Empty>No capabilities were projected.</Empty>
    ) : (
      <ul className="divide-y divide-[var(--color-border)] text-xs">
        {view.sections.capabilities.map((capability) => (
          <li className="flex items-center justify-between gap-2 py-2" key={capability.kind}>
            <span>{capability.kind.replaceAll('_', ' ')}</span>
            <span className="text-[var(--color-text-muted)]">
              {capability.state.replaceAll('_', ' ')}
            </span>
          </li>
        ))}
      </ul>
    );
  }
  if (section === 'usage') {
    return (
      <div className="grid gap-4 py-2 sm:grid-cols-2">
        {view.usage.thisRequest === null ? (
          <div aria-label="This Request">
            <h4 className="text-xs font-semibold">This Request</h4>
            <Empty>Usage unavailable.</Empty>
          </div>
        ) : (
          <UsageScope label="This Request" usage={view.usage.thisRequest} />
        )}
        <UsageScope label="This Task" usage={view.usage.thisTask} />
      </div>
    );
  }
  const health = view.health;
  return (
    <div className="py-2">
      <dl className="grid grid-cols-2 gap-3 text-xs">
        <div>
          <dt className="text-[var(--color-text-muted)]">Level</dt>
          <dd>{health.level}</dd>
        </div>
        {health.context_token_limit === null ? null : (
          <div>
            <dt className="text-[var(--color-text-muted)]">Limit</dt>
            <dd>{health.context_token_limit.toLocaleString('en-US')}</dd>
          </div>
        )}
        {health.context_tokens_remaining === null ? null : (
          <div>
            <dt className="text-[var(--color-text-muted)]">Remaining</dt>
            <dd>{health.context_tokens_remaining.toLocaleString('en-US')}</dd>
          </div>
        )}
        <div>
          <dt className="text-[var(--color-text-muted)]">Source drift</dt>
          <dd>{health.source_drift_count}</dd>
        </div>
      </dl>
      {view.warnings.length === 0 ? null : (
        <ul className="mt-3 divide-y divide-[var(--color-warning-border)] border-y border-[var(--color-warning-border)] bg-[var(--color-warning)] px-2">
          {view.warnings.map((warning, index) => (
            <li className="py-2 text-xs" key={`${warning.code}:${String(index)}`}>
              <p className="font-medium">{warning.code.replaceAll('_', ' ')}</p>
              <SafeMarkdown source={warning.summary} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
