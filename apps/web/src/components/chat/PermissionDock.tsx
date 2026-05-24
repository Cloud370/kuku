import { Button } from "@/components/ui/Button";

export type PermissionDockProps = {
  toolIcon?: string;
  toolName: string;
  riskLabel?: string;
  summary: string;
  onDeny?: () => void;
  onAllowOnce?: () => void;
  onAllowAlways?: () => void;
};

export function PermissionDock({
  toolIcon = "🔧",
  toolName,
  riskLabel,
  summary,
  onDeny,
  onAllowOnce,
  onAllowAlways,
}: PermissionDockProps) {
  return (
    <div className="shrink-0 border-t border-b border-[var(--color-warning-border)] bg-[var(--color-warning)] px-4 py-2.5">
      <div className="flex items-start gap-3">
        <span className="text-[var(--text-lg)] shrink-0 mt-0.5">{toolIcon}</span>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 mb-1">
            <span className="text-[var(--text-sm)] font-medium text-[var(--color-text-primary)]">
              {toolName}
            </span>
            {riskLabel && (
              <span className="text-[var(--text-xs)] text-yellow-600 bg-yellow-100 rounded-[var(--radius-full)] px-2 py-0.5">
                {riskLabel}
              </span>
            )}
          </div>
          <p className="text-[var(--text-xs)] text-[var(--color-text-secondary)] mb-2">
            {summary}
          </p>
          <div className="flex gap-2">
            <Button variant="ghost" size="sm" onClick={onDeny}>
              Deny
            </Button>
            <Button variant="primary" size="sm" onClick={onAllowOnce}>
              Allow Once
            </Button>
            <Button variant="secondary" size="sm" onClick={onAllowAlways}>
              Allow Always
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
