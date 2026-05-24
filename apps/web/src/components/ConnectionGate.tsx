import { useState } from "react";
import { useHealth } from "@/queries/health";
import { Button } from "@/components/ui/Button";

function LoadingScreen() {
  return (
    <div className="flex flex-col items-center justify-center min-h-screen gap-4">
      <div
        className="w-8 h-8 border-2 border-[var(--color-border)] border-t-[var(--color-accent)] rounded-full animate-spin"
        role="status"
      />
      <p className="text-[var(--color-text-muted)] font-mono text-[var(--text-sm)]">
        Connecting to kuku...
      </p>
    </div>
  );
}

function ErrorScreen({
  onRetry,
  isRefetching,
  onSkip,
}: {
  onRetry: () => void;
  isRefetching: boolean;
  onSkip: () => void;
}) {
  return (
    <div className="flex flex-col items-center justify-center min-h-screen gap-4">
      <div className="text-3xl select-none">&#9888;</div>
      <p className="text-[var(--color-text-primary)] text-[var(--text-lg)] font-medium">
        Cannot connect to kuku server
      </p>
      <p className="text-[var(--color-text-muted)] text-[var(--text-sm)]">
        Is kuku-server running on localhost?
      </p>
      <div className="flex gap-2">
        <Button variant="secondary" size="sm" onClick={onRetry} disabled={isRefetching}>
          {isRefetching ? "Retrying..." : "Retry"}
        </Button>
        <Button variant="ghost" size="sm" onClick={onSkip}>
          Skip
        </Button>
      </div>
    </div>
  );
}

export function ConnectionGate({ children }: { children: React.ReactNode }) {
  const { isLoading, isError, refetch, isRefetching } = useHealth();
  const [skipped, setSkipped] = useState(false);

  if (isLoading && !skipped) return <LoadingScreen />;
  if (isError && !skipped)
    return (
      <ErrorScreen
        onRetry={() => { void refetch(); }}
        isRefetching={isRefetching}
        onSkip={() => setSkipped(true)}
      />
    );
  return <>{children}</>;
}
