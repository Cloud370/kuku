const browserTokenKey = "kuku-browser-token";
let explicitToken: string | undefined;
let exchange: Promise<void> | undefined;

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function readToken(): string | null {
  try {
    return sessionStorage.getItem(browserTokenKey);
  } catch {
    return null;
  }
}

export function setAccessToken(token: string) {
  explicitToken = token;
}

export async function request(path: string, init: RequestInit = {}, authenticated = true) {
  const headers = new Headers(init.headers);
  const token = authenticated && (explicitToken ?? readToken());
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const response = await fetch(path, {
    ...init,
    headers,
    credentials: "omit",
    mode: "same-origin",
    redirect: "error",
    cache: "no-store",
  });
  if (response.ok) return response;
  const error = (await response.json().catch(() => null)) as {
    code?: string;
    message?: string;
  } | null;
  throw new ApiError(
    response.status,
    error?.code ?? "http_error",
    error?.message ?? `Server returned HTTP ${String(response.status)}`,
  );
}

function jsonSignal(signal?: AbortSignal) {
  const timeout = AbortSignal.timeout(15_000);
  return signal ? AbortSignal.any([signal, timeout]) : timeout;
}

export async function get<T>(path: string, signal?: AbortSignal, authenticated = true): Promise<T> {
  const response = await request(path, { signal: jsonSignal(signal) }, authenticated);
  return response.json() as Promise<T>;
}

export async function post<T>(path: string, body: unknown, authenticated = true): Promise<T> {
  const response = await request(
    path,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
      signal: jsonSignal(),
    },
    authenticated,
  );
  return response.json() as Promise<T>;
}

export function exchangeBrowserTicket(): Promise<void> {
  if (exchange) return exchange;
  const url = new URL(window.location.href);
  const fragment = new URLSearchParams(url.hash.slice(1));
  const ticket = fragment.get("ticket");
  if (ticket === null) return Promise.resolve();

  fragment.delete("ticket");
  url.hash = fragment.toString();
  window.history.replaceState(window.history.state, "", url);
  exchange = post<{ token: string }>("/service/browser-session", { ticket }, false).then(
    ({ token }) => {
      if (typeof token !== "string" || token.length === 0) {
        throw new Error("The server returned an invalid browser credential.");
      }
      explicitToken = token;
      sessionStorage.setItem(browserTokenKey, token);
    },
  );
  return exchange;
}
