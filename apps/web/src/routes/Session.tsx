import { Card } from "@/components/ui/Card";

export function Session({ id }: { id: string }) {
  return (
    <div className="p-8">
      <p className="text-[var(--color-text-muted)] text-[var(--text-sm)] mb-4">
        Session: {id}
      </p>
      <Card>
        <Card.Body>
          <p className="text-[var(--color-text-secondary)]">
            Session view coming in Plan 2.
          </p>
        </Card.Body>
      </Card>
    </div>
  );
}
