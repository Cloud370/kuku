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

function ErrorScreen({ onRetry, isRefetching }: { onRetry: () => void; isRefetching: boolean }) {
  return (
    <div className="flex flex-col items-center justify-center min-h-screen gap-4">
      <div className="text-3xl select-none">&#9888;</div>
      <p className="text-[var(--color-text-primary)] text-[var(--text-lg)] font-medium">
        Cannot connect to kuku server
      </p>
      <p className="text-[var(--color-text-muted)] text-[var(--text-sm)]">
        Is kuku-server running on localhost?
      </p>
      <Button variant="secondary" size="sm" onClick={onRetry} disabled={isRefetching}>
        {isRefetching ? "Retrying..." : "Retry"}
      </Button>
    </div>
  );
}

export function ConnectionGate({ children }: { children: React.ReactNode }) {
  const { isLoading, isError, refetch, isRefetching } = useHealth();

  if (isLoading) return <LoadingScreen />;
  if (isError) return <ErrorScreen onRetry={() => { void refetch(); }} isRefetching={isRefetching} />;
  return <>{children}</>;
}
