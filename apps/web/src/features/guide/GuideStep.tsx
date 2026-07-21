import type { ReactNode } from 'react';

interface GuideStepProps {
  action: ReactNode;
  description: string;
  title: string;
}

export function GuideStep({ action, description, title }: GuideStepProps) {
  return (
    <li className="grid gap-2 border-b border-[var(--color-border)] py-4 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
      <div>
        <h2 className="text-sm font-semibold">{title}</h2>
        <p className="mt-1 text-sm text-[var(--color-text-secondary)]">{description}</p>
      </div>
      {action}
    </li>
  );
}
