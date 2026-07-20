import { useEffect, useState, type ReactNode, type SyntheticEvent } from 'react';
import { Check, LoaderCircle, ServerCog } from 'lucide-react';

import type {
  CompleteInitRequest,
  InitStatus,
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
}

interface InitScreenProps {
  operations: InitOperations;
  onComplete: () => Promise<void>;
}

interface PhaseFormProps {
  children: ReactNode;
  error: string | null;
  label: string;
  pending: boolean;
  onSubmit: (event: SyntheticEvent<HTMLFormElement, SubmitEvent>) => void;
}

function PhaseForm({ children, error, label, pending, onSubmit }: PhaseFormProps) {
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
        disabled={pending}
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
  const [workspaceRootId, setWorkspaceRootId] = useState('root:default');
  const [workspacePath, setWorkspacePath] = useState('.');
  const [testTierId, setTestTierId] = useState('tier:local:balanced');

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

  async function run(operation: () => Promise<unknown>, finish = false) {
    setPending(true);
    setError(null);
    try {
      await operation();
      const next = await refresh();
      if (finish && next.phase === 'complete') await onComplete();
    } catch {
      setError('This setup step could not be saved. Try again.');
    } finally {
      setPending(false);
    }
  }

  function form(
    label: string,
    children: ReactNode,
    operation: () => Promise<unknown>,
    finish = false,
  ) {
    return (
      <PhaseForm
        error={error}
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
        <Field label="API format" name="provider-format" onChange={setFormat} value={format} />
        <Field label="Base URL" name="base-url" onChange={setBaseUrl} value={baseUrl} />
        <Field
          label="Provider credential"
          name="provider-credential"
          onChange={setCredential}
          type="password"
          value={credential}
        />
        <Field label="Tier ID" name="tier-id" onChange={setTierId} value={tierId} />
        <Field label="Model" name="model" onChange={setModel} value={model} />
      </>,
      () =>
        operations.providers({
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
        }),
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
        <Field
          label="Registration root ID"
          name="workspace-root-id"
          onChange={setWorkspaceRootId}
          value={workspaceRootId}
        />
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
    );
  } else if (status.phase === 'workspace_ready') {
    content = form(
      'Test provider',
      <Field
        label="Test Tier ID"
        name="test-tier-id"
        onChange={setTestTierId}
        value={testTierId}
      />,
      () =>
        operations.test({
          expected_revision: status.server_revision,
          tier_id: testTierId,
        }),
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
