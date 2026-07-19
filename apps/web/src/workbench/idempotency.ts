export function newIdempotencyKey(): string {
  return `idem-${crypto.randomUUID()}`;
}
