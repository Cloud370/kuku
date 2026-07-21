import type { ConnectionInfo, PlatformCatalog, WorkspaceSummary } from '../../api/generated';

export interface SettingsDraft {
  defaultTier: string;
  defaultWorkspaceId: string;
  maxConcurrentRuns: string;
  autoDiscover: boolean;
}

interface SettingsSectionsProps {
  catalog: PlatformCatalog;
  connection: ConnectionInfo;
  draft: SettingsDraft;
  onChange: (draft: SettingsDraft) => void;
  onOpenGuide?: () => void;
  onOpenQr: () => void;
  workspaces: WorkspaceSummary[];
}

export function SettingsSections({
  catalog,
  connection,
  draft,
  onChange,
  onOpenGuide,
  onOpenQr,
  workspaces,
}: SettingsSectionsProps) {
  return (
    <div className="mx-auto grid w-full max-w-4xl gap-6 lg:grid-cols-2">
      <section
        aria-labelledby="connection-heading"
        className="border-b border-[var(--color-border)] pb-5"
      >
        <h2 className="text-sm font-semibold" id="connection-heading">
          Connection
        </h2>
        <dl className="mt-3 grid gap-2 text-sm">
          <div className="flex justify-between gap-4">
            <dt className="text-[var(--color-text-secondary)]">Server</dt>
            <dd>{connection.display_name}</dd>
          </div>
          <div className="flex justify-between gap-4">
            <dt className="text-[var(--color-text-secondary)]">Origin</dt>
            <dd className="min-w-0 break-all text-right">{connection.preferred_origin}</dd>
          </div>
        </dl>
        <button
          className="mt-4 border border-[var(--color-border)] px-3 py-2 text-xs font-medium focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          onClick={onOpenQr}
          type="button"
        >
          Open Connection QR
        </button>
      </section>

      <section
        aria-labelledby="providers-heading"
        className="border-b border-[var(--color-border)] pb-5"
      >
        <h2 className="text-sm font-semibold" id="providers-heading">
          Providers
        </h2>
        <ul className="mt-3 space-y-2 text-sm">
          {catalog.credentials.map((credential) => (
            <li className="flex justify-between gap-3" key={credential.provider_id}>
              <span>{credential.provider_id}</span>
              <span className="text-[var(--color-text-secondary)]">
                {credential.present ? 'Configured' : 'Not configured'}
              </span>
            </li>
          ))}
        </ul>
      </section>

      <section
        aria-labelledby="defaults-heading"
        className="grid gap-4 border-b border-[var(--color-border)] pb-5 lg:col-span-2"
      >
        <h2 className="text-sm font-semibold" id="defaults-heading">
          Defaults
        </h2>
        <label className="grid gap-1 text-xs">
          Default Tier
          <select
            className="h-10 border border-[var(--color-border)] bg-[var(--color-surface-raised)] px-2 text-sm"
            onChange={(event) => {
              onChange({ ...draft, defaultTier: event.currentTarget.value });
            }}
            value={draft.defaultTier}
          >
            {catalog.tiers.map((tier) => (
              <option key={tier.tier_id} value={tier.tier_id}>
                {tier.label}
              </option>
            ))}
          </select>
        </label>
        <label className="grid gap-1 text-xs">
          Default Workspace
          <select
            className="h-10 border border-[var(--color-border)] bg-[var(--color-surface-raised)] px-2 text-sm"
            onChange={(event) => {
              onChange({ ...draft, defaultWorkspaceId: event.currentTarget.value });
            }}
            value={draft.defaultWorkspaceId}
          >
            <option value="">No default Workspace</option>
            {workspaces.map((workspace) => (
              <option key={workspace.workspace_id} value={workspace.workspace_id}>
                {workspace.label}
              </option>
            ))}
          </select>
        </label>
        <label className="grid gap-1 text-xs">
          Maximum concurrent runs
          <input
            aria-label="Maximum concurrent runs"
            className="h-10 border border-[var(--color-border)] bg-[var(--color-surface-raised)] px-2 text-sm"
            min={1}
            onChange={(event) => {
              onChange({ ...draft, maxConcurrentRuns: event.currentTarget.value });
            }}
            type="number"
            value={draft.maxConcurrentRuns}
          />
        </label>
        <label className="flex items-center gap-2 text-xs">
          <input
            checked={draft.autoDiscover}
            onChange={(event) => {
              onChange({ ...draft, autoDiscover: event.currentTarget.checked });
            }}
            type="checkbox"
          />
          Discover Skills and Agents automatically
        </label>
      </section>

      {onOpenGuide === undefined ? null : (
        <button
          className="w-fit text-sm font-medium text-[var(--color-accent)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          onClick={onOpenGuide}
          type="button"
        >
          Open Guide
        </button>
      )}
    </div>
  );
}
