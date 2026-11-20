import { useQuery } from "@tanstack/react-query";
import { fetchModels } from "@/api/sessions";
import type { Snapshot } from "@/api/sessions";

export function ModelControl({
  snapshot,
  tier,
  onChange,
  disabled,
}: {
  snapshot: Snapshot | null;
  tier: string;
  onChange: (tier: string) => void;
  disabled?: boolean;
}) {
  const models = useQuery({
    queryKey: ["models"],
    queryFn: ({ signal }) => fetchModels(signal),
    staleTime: 30_000,
  });
  const options = models.data?.tiers ?? [];
  const defaultTier = models.data?.default_tier;
  const value = snapshot ? (snapshot.agent.config.tier ?? "") : tier;
  const selection = snapshot?.selection;
  return (
    <label className="inline-flex flex-wrap items-center gap-2">
      {selection && <span title={selection.connection}>{selection.model}</span>}
      {snapshot?.pending_selection && (
        <span title={snapshot.pending_selection.connection}>
          Pending {snapshot.pending_selection.model}
        </span>
      )}
      <select
        aria-label="Model tier"
        value={value}
        disabled={disabled || models.isPending || models.isError}
        onChange={(event) => {
          onChange(event.target.value);
        }}
        className="rounded border border-[var(--color-border)] bg-[var(--color-surface-raised)] px-1.5 py-1 text-xs"
      >
        <option value="">{defaultTier ? `Default (${defaultTier})` : "Server default"}</option>
        {value && !options.some(({ name }) => name === value) && (
          <option value={value}>{value}</option>
        )}
        {options.map((option) => (
          <option
            key={option.name}
            value={option.name}
            disabled={!!option.error}
            title={option.error?.message}
          >
            {option.name} · {option.model}
          </option>
        ))}
      </select>
    </label>
  );
}
