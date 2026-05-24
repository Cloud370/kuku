import type { ReactNode } from "react";

type ErrorType = "provider_network" | "provider_auth" | "provider_rate_limit" | "provider_overflow" | "internal";

const errorLabels: Record<ErrorType, string> = {
  provider_network: "Network error — check your connection",
  provider_auth: "Authentication failed — check your API key",
  provider_rate_limit: "Rate limit exceeded — wait before retrying",
  provider_overflow: "Context limit exceeded",
  internal: "Internal error",
};

export type ErrorNoticeProps = {
  type?: ErrorType;
  message?: string;
  children?: ReactNode;
};

export function ErrorNotice({ type = "internal", message, children }: ErrorNoticeProps) {
  return (
    <div className="my-2 rounded-[var(--radius-md)] border border-[var(--color-error-border)] bg-[var(--color-error)] overflow-hidden">
      <div className="flex items-start gap-2 px-3 py-2">
        <span className="text-[var(--text-sm)] shrink-0">&#x26A0;</span>
        <div className="flex-1 min-w-0">
          <p className="text-[var(--text-sm)] text-red-400 font-medium">
            {errorLabels[type]}
          </p>
          {message && (
            <p className="text-[var(--text-xs)] text-red-400/70 mt-0.5">{message}</p>
          )}
          {children}
        </div>
      </div>
    </div>
  );
}
