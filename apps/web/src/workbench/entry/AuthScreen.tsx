import { useState, type SyntheticEvent } from 'react';
import { KeyRound } from 'lucide-react';

interface AuthScreenProps {
  serverName: string;
  onSubmit: (credential: string) => Promise<void>;
}

export function AuthScreen({ serverName, onSubmit }: AuthScreenProps) {
  const [credential, setCredential] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: SyntheticEvent<HTMLFormElement, SubmitEvent>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      await onSubmit(credential);
    } catch {
      setError('Credential was rejected. Check it and try again.');
      setCredential('');
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <main className="flex min-h-dvh items-center justify-center bg-[var(--color-surface)] p-6">
      <section
        aria-labelledby="auth-title"
        className="w-full max-w-md border border-[var(--color-border)] bg-[var(--color-surface-raised)] p-6"
      >
        <KeyRound aria-hidden="true" className="mb-4 text-[var(--color-accent)]" size={24} />
        <h1 id="auth-title" className="text-xl font-semibold">
          Connect to {serverName}
        </h1>
        <p className="mt-2 text-sm text-[var(--color-text-secondary)]">
          Enter the bearer credential issued by this server.
        </p>
        <form className="mt-6 space-y-4" onSubmit={(event) => void submit(event)}>
          <label className="block text-sm font-medium" htmlFor="entry-credential">
            Credential
          </label>
          <input
            autoComplete="current-password"
            className="w-full rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)] px-3 py-2 text-sm outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)]"
            id="entry-credential"
            onChange={(event) => {
              setCredential(event.target.value);
            }}
            required
            type="password"
            value={credential}
          />
          {error !== null ? (
            <p className="text-sm text-[var(--color-error)]" role="alert">
              {error}
            </p>
          ) : null}
          <button
            className="inline-flex w-full items-center justify-center gap-2 rounded-[var(--radius-md)] bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-white disabled:opacity-40"
            disabled={submitting || credential.length === 0}
            type="submit"
          >
            <KeyRound aria-hidden="true" size={16} />
            {submitting ? 'Connecting' : 'Connect'}
          </button>
        </form>
      </section>
    </main>
  );
}
