import { useEffect, useRef, useState } from 'react';

import type { RunState, TaskProjection } from '../../api/generated';

const runAnnouncements: Record<RunState, string> = {
  completed: 'Run completed',
  failed: 'Run failed',
  interrupted: 'Run interrupted',
  needs_attention: 'Run needs attention',
  queued: 'Run queued',
  running: 'Run started',
  stopped: 'Run stopped',
  stopping: 'Run stopping',
};

function currentState(projection: TaskProjection | null): RunState | null {
  return projection?.active_run?.state ?? projection?.latest_run?.state ?? null;
}

function pendingInteractionCount(projection: TaskProjection | null): number {
  return (
    projection?.timeline.filter(
      (item) => item.type === 'interaction' && item.item.status === 'pending',
    ).length ?? 0
  );
}

interface RunLiveRegionProps {
  projection: TaskProjection | null;
}

export function RunLiveRegion({ projection }: RunLiveRegionProps) {
  const state = currentState(projection);
  const pendingCount = pendingInteractionCount(projection);
  const previousState = useRef<RunState | null>(null);
  const previousPendingCount = useRef(0);
  const [announcement, setAnnouncement] = useState(() =>
    state === null ? '' : runAnnouncements[state],
  );

  useEffect(() => {
    if (state !== null && state !== previousState.current) {
      setAnnouncement(runAnnouncements[state]);
    } else if (pendingCount > previousPendingCount.current) {
      setAnnouncement('Run needs attention');
    }
    previousState.current = state;
    previousPendingCount.current = pendingCount;
  }, [pendingCount, state]);

  return (
    <div aria-label="Run status" aria-live="polite" className="sr-only" role="status">
      {announcement}
    </div>
  );
}
