import { Card } from "@/components/ui/Card";

export function Home() {
  return (
    <div className="p-8">
      <h1 className="text-[var(--text-xl)] font-semibold text-[var(--color-text-primary)] mb-6">
        Sessions
      </h1>
      <div className="grid gap-4 max-w-2xl">
        <Card>
          <Card.Body>
            <p className="text-[var(--color-text-muted)]">
              No sessions yet. Start a new one from the workspace.
            </p>
          </Card.Body>
        </Card>
      </div>
    </div>
  );
}
