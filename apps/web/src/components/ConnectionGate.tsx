import { useState } from "react";
import type { SubmitEvent, ReactNode } from "react";
import { ApiError, errorMessage, setAccessToken } from "@/api/client";
import { useHealth } from "@/queries/health";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";

export function ConnectionGate({ children }: { children: ReactNode }) {
  const { data, error, isFetching, refetch } = useHealth();
  const [token, setToken] = useState("");
  if (data && !error && !data.mismatch) return <>{children}</>;

  if (data?.mismatch) {
    return (
      <div className="min-h-screen flex flex-col items-center justify-center gap-4 px-6 text-center">
        <h1 className="text-lg font-medium">Incompatible server protocol</h1>
        <p role="alert" className="text-sm">
          {data.mismatch}
        </p>
        {data.draining && (
          <p role="status" className="text-sm">
            Server restart is in progress.
          </p>
        )}
        {data.busy.length > 0 && <p className="text-sm">Working Agents · {data.busy.join(", ")}</p>}
        {data.instance_id && (
          <p className="text-sm">
            Use <code>kuku server restart</code>, then refresh this page.
          </p>
        )}
        <Button
          variant="secondary"
          size="sm"
          onClick={() => {
            void refetch();
          }}
          disabled={isFetching}
        >
          Refresh status
        </Button>
      </div>
    );
  }

  const authRequired = error instanceof ApiError && error.status === 401;
  function authenticate(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!token.trim()) return;
    setAccessToken(token.trim());
    setToken("");
    void refetch();
  }

  return (
    <div className="min-h-screen flex flex-col items-center justify-center gap-4 px-6 text-center">
      {error ? (
        <>
          <h1 className="text-lg font-medium">
            {authRequired ? "Connect to kuku" : "Cannot connect to kuku"}
          </h1>
          <p role="alert" className="max-w-xl text-sm text-[var(--color-text-muted)]">
            {errorMessage(error)}
          </p>
          {authRequired && (
            <>
              <p className="text-sm text-[var(--color-text-muted)]">
                Use an authenticated development link or enter the server access token.
              </p>
              {window.isSecureContext && (
                <form className="flex gap-2" onSubmit={authenticate}>
                  <Input
                    aria-label="Server access token"
                    placeholder="Server access token"
                    type="password"
                    autoComplete="off"
                    value={token}
                    onChange={(event) => {
                      setToken(event.target.value);
                    }}
                  />
                  <Button type="submit" disabled={isFetching || !token.trim()}>
                    Connect
                  </Button>
                </form>
              )}
            </>
          )}
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              void refetch();
            }}
            disabled={isFetching}
          >
            {isFetching ? "Connecting…" : "Retry connection"}
          </Button>
        </>
      ) : (
        <p role="status" className="text-sm text-[var(--color-text-muted)]">
          Connecting to kuku…
        </p>
      )}
    </div>
  );
}
