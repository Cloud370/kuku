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
  const defaultRoute = options.find(({ name }) => name === (value || defaultTier));
  const active = selection ?? defaultRoute;
  const capabilities = active?.capabilities;
  const source = (value: string) => value.replaceAll("_", " ");
  const describe = (field?: { value: string[]; source: string } | null) =>
    field ? `${field.value.join(", ") || "none"} (${source(field.source)})` : "Unknown";
  return (
    <div className="inline-flex flex-wrap items-center gap-2">
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
      {capabilities && (
        <details className="text-xs">
          <summary>Model capabilities</summary>
          <dl>
            <dt>Thinking</dt>
            <dd>{active.settings?.reasoning_effort ?? "Provider default (omit effort)"}</dd>
            <dt>Upstream default</dt>
            <dd>{capabilities.reasoning_default?.value ?? "Unknown"}</dd>
            <dt>Native efforts</dt>
            <dd>{describe(capabilities.reasoning_source ? {
              value: capabilities.reasoning_efforts,
              source: capabilities.reasoning_source,
            } : null)}</dd>
            <dt>Declared input</dt>
            <dd>{describe(capabilities.input_modalities)}</dd>
            <dt>Declared output</dt>
            <dd>{describe(capabilities.output_modalities)}</dd>
            <dt>Advertised interfaces</dt>
            <dd>{describe(capabilities.interfaces)}</dd>
            <dt>Agent requested output</dt>
            <dd>{active.output ?? "text"}</dd>
            <dt>Adapter media support</dt>
            <dd>Text only</dd>
          </dl>
        </details>
      )}
    </div>
  );
}
