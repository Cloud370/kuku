import { Button } from "@/components/ui/Button";

export type PermissionDockProps = {
  toolIcon?: string;
  toolName: string;
  riskLabel?: string;
  requestId?: string;
  summary: string;
  detail?: string;
  disabled?: boolean;
  onDeny?: () => void;
  onAllowOnce?: () => void;
};

export function PermissionDock({
  toolIcon = "🔧",
  toolName,
  riskLabel,
  requestId,
  summary,
  detail,
  disabled,
  onDeny,
  onAllowOnce,
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
            {requestId && (
              <span className="text-xs text-[var(--color-text-muted)]">{requestId}</span>
            )}
          </div>
          <p className="text-[var(--text-xs)] text-[var(--color-text-secondary)] mb-2">{summary}</p>
          {detail && (
            <pre className="max-h-32 overflow-auto text-xs whitespace-pre-wrap mb-2">{detail}</pre>
          )}
          <div className="flex gap-2">
            <Button variant="ghost" size="sm" onClick={onDeny} disabled={disabled}>
              Deny
            </Button>
            <Button variant="primary" size="sm" onClick={onAllowOnce} disabled={disabled}>
              Allow Once
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
