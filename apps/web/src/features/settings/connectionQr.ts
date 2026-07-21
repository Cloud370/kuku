import type { ConnectionInfo } from '../../api/generated';

export function buildCredentialQrValue(info: ConnectionInfo, credential: string): string {
  const url = new URL(info.preferred_origin);
  url.hash = `credential=${encodeURIComponent(credential)}`;
  return url.toString();
}
