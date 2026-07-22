import { useEffect, useState, type ReactNode, type SyntheticEvent } from 'react';
import { Check, LoaderCircle, ServerCog } from 'lucide-react';

import type {
  CompleteInitRequest,
  InitStatus,
  PlatformCatalog,
  RegistrationRootSummary,
  RegisterInitialWorkspaceRequest,
  TestProviderRequest,
  TestProviderResult,
  UpdateDefaultTierRequest,
  UpdateProvidersRequest,
} from '../../api/generated';

export interface InitOperations {
  status: () => Promise<InitStatus>;
  providers: (body: UpdateProvidersRequest) => Promise<InitStatus>;
  defaultTier: (body: UpdateDefaultTierRequest) => Promise<InitStatus>;
  workspace: (body: RegisterInitialWorkspaceRequest) => Promise<InitStatus>;
  test: (body: TestProviderRequest) => Promise<TestProviderResult>;
  complete: (body: CompleteInitRequest) => Promise<InitStatus>;
  catalog: () => Promise<PlatformCatalog>;
  registrationRoots: () => Promise<{ items: RegistrationRootSummary[] }>;
}

interface InitScreenProps {
  operations: InitOperations;
  onComplete: () => Promise<void>;
}

interface PhaseFormProps {
  children: ReactNode;
  disabled?: boolean;
  error: string | null;
  label: string;
  pending: boolean;
  onSubmit: (event: SyntheticEvent<HTMLFormElement, SubmitEvent>) => void;
}

function PhaseForm({ children, disabled = false, error, label, pending, onSubmit }: PhaseFormProps) {
  return (
    <form className="mt-6 space-y-4" onSubmit={onSubmit}>
      {children}
      {error !== null ? (
        <p className="text-sm text-[var(--color-error)]" role="alert">
          {error}
        </p>
      ) : null}
      <button
        className="w-full rounded-[var(--radius-md)] bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-white disabled:opacity-40"
        disabled={disabled || pending}
        type="submit"
      >
        {pending ? 'Saving' : label}
      </button>
    </form>
  );
}

function Field({
  label,
  name,
  onChange,
  type = 'text',
  value,
}: {
  label: string;
  name: string;
  onChange: (value: string) => void;
  type?: 'password' | 'text';
  value: string;
}) {
  return (
    <label className="block space-y-2 text-sm font-medium">
      <span>{label}</span>
      <input
        aria-label={label}
        className="w-full rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)] px-3 py-2 text-sm outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)]"
        name={name}
        onChange={(event) => {
          onChange(event.target.value);
        }}
        required
        type={type}
        value={value}
      />
    </label>
  );
}

function ApiFormatSelect({ onChange, value }: { onChange: (value: string) => void; value: string }) {
  return (
    <label className="block space-y-2 text-sm font-medium">
      <span>API format</span>
      <select
        aria-label="API format"
        className="w-full rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)] px-3 py-2 text-sm outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)]"
        name="provider-format"
        onChange={(event) => {
          onChange(event.target.value);
        }}
        required
        value={value}
      >
        <option disabled value="">
          Select API format
        </option>
        <option value="openai-responses">OpenAI Responses</option>
        <option value="openai-chat">OpenAI Chat Completions</option>
        <option value="anthropic">Anthropic Messages</option>
      </select>
    </label>
  );
}

function providerEndpoint(format: string, baseUrl: string): string {
  const base = baseUrl.replace(/\/+$/u, '');
  if (base.length === 0) return '';
  if (format === 'anthropic') return `${base}${base.endsWith('/v1') ? '' : '/v1'}/messages`;
  if (format === 'openai-chat') return `${base}/chat/completions`;
  if (format === 'openai-responses') return `${base}/responses`;
  return '';
}

function WorkspaceRootSelect({
  disabled,
  onChange,
  roots,
  value,
}: {
  disabled: boolean;
  onChange: (value: string) => void;
  roots: RegistrationRootSummary[];
  value: string;
}) {
  return (
    <label className="block space-y-2 text-sm font-medium">
      <span>Workspace Root</span>
      <select
        aria-label="Workspace Root"
        className="w-full rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)] px-3 py-2 text-sm outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)] disabled:opacity-40"
        disabled={disabled}
        name="workspace-root"
        onChange={(event) => {
          onChange(event.target.value);
        }}
        required
        value={value}
      >
        {roots.length === 0 ? <option value="">Loading workspace roots</option> : null}
        {roots.map((root) => (
          <option key={root.root_id} value={root.root_id}>
            {root.label}
          </option>
        ))}
      </select>
    </label>
  );
}

export function InitScreen({ operations, onComplete }: InitScreenProps) {
  const [status, setStatus] = useState<InitStatus | null>(null);
  const [loadingError, setLoadingError] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [providerId, setProviderId] = useState('');
  const [format, setFormat] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [credential, setCredential] = useState('');
  const [tierId, setTierId] = useState('');
  const [model, setModel] = useState('');
  const [defaultTierId, setDefaultTierId] = useState('tier:local:balanced');
  const [workspaceLabel, setWorkspaceLabel] = useState('Workspace');
  const [workspaceRootId, setWorkspaceRootId] = useState('');
  const [workspacePath, setWorkspacePath] = useState('.');
  const [testTierId, setTestTierId] = useState('');
  const [catalog, setCatalog] = useState<PlatformCatalog | null>(null);
  const [workspaceRoots, setWorkspaceRoots] = useState<RegistrationRootSummary[]>([]);
  const [workspaceRootsError, setWorkspaceRootsError] = useState(false);

  async function refresh() {
    const next = await operations.status();
    setStatus(next);
    setLoadingError(false);
    return next;
  }

  useEffect(() => {
    let active = true;
    operations.status().then(
      (next) => {
        if (active) setStatus(next);
      },
      () => {
        if (active) setLoadingError(true);
      },
    );
    return () => {
      active = false;
    };
  }, [operations]);

  useEffect(() => {
    if (status?.phase !== 'default_tier_ready') return;
    let active = true;
    operations.registrationRoots().then(
      (page) => {
        if (!active) return;
        setWorkspaceRoots(page.items);
        setWorkspaceRootsError(false);
        setWorkspaceRootId((current) =>
          page.items.some((root) => root.root_id === current)
            ? current
            : (page.items[0]?.root_id ?? ''),
        );
      },
      () => {
        if (active) setWorkspaceRootsError(true);
      },
    );
    return () => {
      active = false;
    };
  }, [operations, status?.phase]);

  useEffect(() => {
    if (status?.phase !== 'workspace_ready') return;
    let active = true;
    operations.catalog().then(
      (next) => {
        if (!active) return;
        setCatalog(next);
        setTestTierId((current) =>
          next.tiers.some((tier) => tier.tier_id === current)
            ? current
            : next.default_tier.tier_id || (next.tiers[0]?.tier_id ?? ''),
        );
      },
      () => {
        if (active) setError('Configured Tiers could not be loaded. Try again.');
      },
    );
    return () => {
      active = false;
    };
  }, [operations, status?.phase]);

  async function run(operation: () => Promise<unknown>, finish = false) {
    setPending(true);
    setError(null);
    try {
      await operation();
      const next = await refresh();
      if (finish && next.phase === 'complete') await onComplete();
    } catch (operationError) {
      setError(
        operationError instanceof Error
          ? operationError.message
          : 'This setup step could not be saved. Try again.',
      );
    } finally {
      setPending(false);
    }
  }

  function form(
    label: string,
    children: ReactNode,
    operation: () => Promise<unknown>,
    finish = false,
    disabled = false,
  ) {
    return (
      <PhaseForm
        error={error}
        disabled={disabled}
        label={label}
        onSubmit={(event) => {
          event.preventDefault();
          void run(operation, finish);
        }}
        pending={pending}
      >
        {children}
      </PhaseForm>
    );
  }

  let content: ReactNode;
  if (loadingError) {
    content = (
      <div className="mt-6" role="alert">
        <p>Setup status is unavailable.</p>
        <button
          className="mt-4 rounded-[var(--radius-md)] border border-[var(--color-border)] px-4 py-2 text-sm font-medium"
          onClick={() => void refresh()}
          type="button"
        >
          Retry
        </button>
      </div>
    );
  } else if (status === null) {
    content = (
      <p className="mt-6 flex items-center gap-2" role="status">
        <LoaderCircle aria-hidden="true" className="animate-spin" size={16} />
        Loading setup
      </p>
    );
  } else if (status.phase === 'required') {
    content = form(
      'Save providers',
      <>
        <Field label="Provider ID" name="provider-id" onChange={setProviderId} value={providerId} />
        <ApiFormatSelect onChange={setFormat} value={format} />
        <Field label="Base URL" name="base-url" onChange={setBaseUrl} value={baseUrl} />
        {providerEndpoint(format, baseUrl) !== '' ? (
          <p className="text-sm text-[var(--color-text-secondary)]">
            Final endpoint:{' '}
            <code className="break-all">{providerEndpoint(format, baseUrl)}</code>
          </p>
        ) : null}
        <Field
          label="API Key"
          name="api-key"
          onChange={setCredential}
          type="password"
          value={credential}
        />
        <Field label="Tier ID" name="tier-id" onChange={setTierId} value={tierId} />
        <Field label="Model" name="model" onChange={setModel} value={model} />
      </>,
      async () => {
        const result = await operations.providers({
          expected_revision: status.server_revision,
          providers: [
            {
              base_url: baseUrl,
              credential: { source: 'direct_value', value: credential },
              format,
              provider_id: providerId,
            },
          ],
          tiers: [
            {
              model,
              provider_id: providerId,
              purpose: 'General use',
              think: null,
              tier_id: tierId,
            },
          ],
        });
        setDefaultTierId(tierId);
        return result;
      },
    );
  } else if (status.phase === 'providers_ready') {
    content = form(
      'Save default Tier',
      <Field
        label="Default Tier ID"
        name="default-tier-id"
        onChange={setDefaultTierId}
        value={defaultTierId}
      />,
      () =>
        operations.defaultTier({
          expected_revision: status.server_revision,
          tier_id: defaultTierId,
        }),
    );
  } else if (status.phase === 'default_tier_ready') {
    content = form(
      'Register workspace',
      <>
        <Field
          label="Workspace label"
          name="workspace-label"
          onChange={setWorkspaceLabel}
          value={workspaceLabel}
        />
        <WorkspaceRootSelect
          disabled={workspaceRoots.length === 0 || workspaceRootsError}
          onChange={setWorkspaceRootId}
          roots={workspaceRoots}
          value={workspaceRootId}
        />
        {workspaceRootsError ? (
          <p className="text-sm text-[var(--color-error)]" role="alert">
            Workspace roots are unavailable. Try again.
          </p>
        ) : null}
        <Field
          label="Workspace path"
          name="workspace-path"
          onChange={setWorkspacePath}
          value={workspacePath}
        />
      </>,
      () =>
        operations.workspace({
          workspace: {
            expected_revision: status.server_revision,
            label: workspaceLabel,
            relative_path: workspacePath,
            root_id: workspaceRootId,
          },
        }),
      false,
      workspaceRootId === '' || workspaceRootsError,
    );
  } else if (status.phase === 'workspace_ready') {
    content = form(
      'Test provider',
      <label className="block space-y-2 text-sm font-medium">
        <span>Test Tier</span>
        <select
          aria-label="Test Tier"
          className="w-full rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)] px-3 py-2 text-sm outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)] disabled:opacity-40"
          disabled={catalog === null || catalog.tiers.length === 0}
          onChange={(event) => {
            setTestTierId(event.target.value);
          }}
          value={testTierId}
        >
          {catalog?.tiers.map((tier) => (
            <option key={tier.tier_id} value={tier.tier_id}>
              {tier.label || tier.tier_id}
              {tier.is_default ? ' (default)' : ''}
            </option>
          ))}
        </select>
      </label>,
      () => {
        return operations.test({
          expected_revision: status.server_revision,
          tier_id: testTierId,
        });
      },
      false,
      testTierId === '',
    );
  } else if (status.phase === 'probe_passed') {
    content = form(
      'Complete setup',
      <p className="flex items-center gap-2 text-sm text-[var(--color-text-secondary)]">
        <Check aria-hidden="true" size={16} />
        Provider and workspace checks passed.
      </p>,
      () => operations.complete({ expected_revision: status.server_revision }),
      true,
    );
  } else {
    content = (
      <p className="mt-6" role="status">
        Finalizing setup
      </p>
    );
  }

  return (
    <main className="flex min-h-dvh items-center justify-center bg-[var(--color-surface)] p-6">
      <section
        aria-labelledby="init-title"
        className="w-full max-w-lg border border-[var(--color-border)] bg-[var(--color-surface-raised)] p-6"
      >
        <ServerCog aria-hidden="true" className="mb-4 text-[var(--color-accent)]" size={24} />
        <h1 id="init-title" className="text-xl font-semibold">
          Initialize kuku
        </h1>
        <p className="mt-2 text-sm text-[var(--color-text-secondary)]">
          Complete the server configuration before opening the Workbench.
        </p>
        {content}
      </section>
    </main>
  );
}
