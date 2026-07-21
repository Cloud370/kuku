import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';

import { webApi } from '../../api/client';
import type { PlatformCatalog } from '../../api/generated';
import { ResponsiveFeatureSurface } from '../experience/ResponsiveFeatureSurface';
import {
  GuideAction,
  firstTaskPrefill,
  guideActionRoute,
  type FirstTaskPrefill,
} from './guideActions';
import { GuideStep } from './GuideStep';

interface GuideRouteApi {
  catalog: { platform: () => Promise<PlatformCatalog> };
  tasks?: { submitRun: unknown };
}

interface GuideRouteProps {
  api?: GuideRouteApi;
  onAction?: (action: GuideAction, route: string) => void;
  onPrefillFirstTask: (prefill: FirstTaskPrefill) => void;
}

const steps: ReadonlyArray<{
  action: GuideAction;
  button: string;
  description: string;
  title: string;
}> = [
  {
    action: GuideAction.Init,
    button: 'Open Init',
    description: 'Complete server initialization when setup is incomplete.',
    title: 'Initialization',
  },
  {
    action: GuideAction.Settings,
    button: 'Open Settings',
    description: 'Review server defaults and connection status.',
    title: 'Settings',
  },
  {
    action: GuideAction.SkillPicker,
    button: 'Choose Skills',
    description: 'Stage workspace Skills before a Run.',
    title: 'Skills',
  },
  {
    action: GuideAction.Context,
    button: 'Inspect Context',
    description: 'Inspect current and historical Request context.',
    title: 'Context',
  },
  {
    action: GuideAction.AgentThread,
    button: 'Open Agent thread',
    description: 'Read delegated Agent activity without sending input.',
    title: 'Agent thread',
  },
  {
    action: GuideAction.Files,
    button: 'Open Files',
    description: 'Inspect workspace-relative files and Changes.',
    title: 'Files and Changes',
  },
  {
    action: GuideAction.ReviewChanges,
    button: 'Prepare Review',
    description: 'Draft review notes before an explicit submission.',
    title: 'Review',
  },
  {
    action: GuideAction.ConnectionQr,
    button: 'Open Connection QR',
    description: 'Connect another device using this browser credential.',
    title: 'Connection',
  },
];

export function GuideRoute({ api = webApi, onAction, onPrefillFirstTask }: GuideRouteProps) {
  const [dismissed, setDismissed] = useState(false);
  const catalog = useQuery({
    queryKey: ['platform-catalog'],
    queryFn: () => api.catalog.platform(),
  });
  return (
    <ResponsiveFeatureSurface
      kind="guide"
      mode="full"
      toolbar={
        <div className="flex w-full items-center justify-between gap-3">
          <h1 className="text-sm font-semibold">Guide</h1>
          <button
            className="text-xs font-medium text-[var(--color-text-secondary)]"
            onClick={() => {
              setDismissed(true);
            }}
            type="button"
          >
            Dismiss Guide
          </button>
        </div>
      }
    >
      {catalog.isPending ? <p role="status">Loading Guide</p> : null}
      {catalog.isError ? <p role="alert">Guide could not be loaded.</p> : null}
      {dismissed ? (
        <div className="grid place-items-center gap-3 py-12 text-center">
          <p>Guide dismissed.</p>
          <button
            className="border border-[var(--color-border)] px-3 py-2 text-xs font-medium"
            onClick={() => {
              setDismissed(false);
            }}
            type="button"
          >
            Show Guide
          </button>
        </div>
      ) : catalog.data === undefined ? null : (
        <div className="mx-auto max-w-3xl">
          <p className="text-sm text-[var(--color-text-secondary)]">
            Default Tier:{' '}
            <span className="text-[var(--color-text-primary)]">
              {catalog.data.default_tier.label}
            </span>
          </p>
          <div className="mt-4 border-y border-[var(--color-border)] py-4">
            <button
              className="bg-[var(--color-accent)] px-3 py-2 text-sm font-medium text-[var(--color-accent-contrast)]"
              onClick={() => {
                onPrefillFirstTask(firstTaskPrefill);
                onAction?.(GuideAction.FirstTask, guideActionRoute(GuideAction.FirstTask));
              }}
              type="button"
            >
              Prepare first task
            </button>
          </div>
          <ol>
            {steps.map((step) => (
              <GuideStep
                action={
                  <button
                    className="border border-[var(--color-border)] px-3 py-2 text-xs font-medium"
                    onClick={() => {
                      onAction?.(step.action, guideActionRoute(step.action));
                    }}
                    type="button"
                  >
                    {step.button}
                  </button>
                }
                description={step.description}
                key={step.action}
                title={step.title}
              />
            ))}
          </ol>
        </div>
      )}
    </ResponsiveFeatureSurface>
  );
}
