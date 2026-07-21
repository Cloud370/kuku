import type { ReactNode } from 'react';

import styles from './ResponsiveFeatureSurface.module.css';

export type FeatureKind = 'context' | 'review' | 'settings' | 'guide';
export type FeaturePresentationMode = 'panel' | 'sheet' | 'full';
export type FeatureTransitionKind = 'idle' | 'loading' | 'error' | 'saving' | 'submitting';

interface FeatureTransitionStatus {
  kind: FeatureTransitionKind;
  message: string;
}

interface ResponsiveFeatureSurfaceProps {
  children: ReactNode;
  kind: FeatureKind;
  mode: FeaturePresentationMode;
  status?: FeatureTransitionStatus;
  toolbar: ReactNode;
}

const labels: Record<FeatureKind, string> = {
  context: 'Agent Context',
  guide: 'Guide',
  review: 'Review',
  settings: 'Settings',
};

export function ResponsiveFeatureSurface({
  children,
  kind,
  mode,
  status,
  toolbar,
}: ResponsiveFeatureSurfaceProps) {
  const announce = status !== undefined && status.kind !== 'idle';
  return (
    <section
      aria-label={labels[kind]}
      className={styles.surface}
      data-feature-kind={kind}
      data-presentation-mode={mode}
    >
      <div className={styles.toolbar} role="toolbar">
        {toolbar}
      </div>
      <div className={styles.content}>{children}</div>
      {announce ? (
        <div aria-atomic="true" aria-live="polite" className={styles.status} role="status">
          {status.message}
        </div>
      ) : null}
    </section>
  );
}
