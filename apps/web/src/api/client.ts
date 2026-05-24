export async function get<T>(path: string): Promise<T> {
  const res = await fetch(path);
  if (!res.ok) throw new Error(`GET ${path}: ${String(res.status)}`);
  return res.json() as Promise<T>;
}

export function postStream(
  path: string,
  body: unknown,
): Promise<ReadableStream<Uint8Array>> {
  return fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  }).then((r) => {
    if (!r.ok || !r.body) throw new Error(`POST ${path}: ${String(r.status)}`);
    return r.body;
  });
}
