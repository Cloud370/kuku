import { useQuery } from "@tanstack/react-query";
import { fetchModels } from "@/api/sessions";
import type { ModelRef, Snapshot } from "@/api/sessions";

function routeKey({ connection, model }: ModelRef) {
  return JSON.stringify([connection, model]);
}

export function ModelControl({
  snapshot,
  draftModel,
  onChange,
  disabled,
}: {
  snapshot: Snapshot | null;
  draftModel?: ModelRef;
  onChange: (model?: ModelRef) => void;
  disabled?: boolean;
}) {
  const models = useQuery({
    queryKey: ["models"],
    queryFn: ({ signal }) => fetchModels(signal),
    staleTime: 30_000,
  });
  const options = models.data?.models ?? [];
  const defaultModel = models.data?.default_model;
  const config = snapshot?.agent.config;
  const selected = config?.connection && config.model
    ? { connection: config.connection, model: config.model }
    : draftModel;
  const value = selected ? routeKey(selected) : "";
  const selection = snapshot?.selection;
  const fallback = selected ?? defaultModel;
  const defaultRoute = fallback && options.find((option) => routeKey(option) === routeKey(fallback));
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
        aria-label="Model"
        value={value}
        disabled={disabled || models.isPending || models.isError}
        onChange={(event) => {
          const option = options.find((option) => routeKey(option) === event.target.value);
          onChange(option ? { connection: option.connection, model: option.model } : undefined);
        }}
        className="rounded border border-[var(--color-border)] bg-[var(--color-surface-raised)] px-1.5 py-1 text-xs"
      >
        {(!snapshot || !selected) && (
          <option value="">
            {defaultModel ? `Default (${defaultModel.connection} · ${defaultModel.model})` : "Server default"}
          </option>
        )}
        {selected && !options.some((option) => routeKey(option) === value) && (
          <option value={value} disabled>{selected.connection} · {selected.model}</option>
        )}
        {options.map((option) => (
          <option
            key={routeKey(option)}
            value={routeKey(option)}
            disabled={!!option.error}
            title={option.error?.message}
          >
            {option.connection} · {option.model}
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
