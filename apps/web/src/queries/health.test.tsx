import { readFileSync } from 'node:fs';
import { URL as FileURL } from 'node:url';
import type { PropsWithChildren } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useHealth } from './health';

// Read the version declared by each Rust endpoint of the shared protocol,
// rather than freezing another number in the browser test fixture.
function rustProtocolVersion(path: string) {
  const source = readFileSync(new FileURL(path, import.meta.url), 'utf8');
  const version = /^pub const SESSION_PROTOCOL_VERSION: u32 = (\d+);$/m.exec(source)?.[1];
  if (version === undefined) throw new Error(`Missing Session protocol version in ${path}`);
  return Number(version);
}

const serverProtocol = rustProtocolVersion('../../../../crates/kuku-server/src/service/mod.rs');
const clientProtocol = rustProtocolVersion('../../../../crates/kuku-client/src/local.rs');

function healthQuery(protocol: number) {
  const service = { version: 'fixture', protocol_version: protocol, management_version: 1 };
  vi.stubGlobal(
    'fetch',
    vi.fn((path: string) => {
      if (path === '/health') return Promise.resolve(Response.json({ ok: true, ...service }));
      if (path === '/service/status')
        return Promise.resolve(
          Response.json({
            ...service,
            config_path: null,
            instance_id: null,
            busy: [],
            draining: false,
          }),
        );
      throw new Error(`Unexpected health request ${path}`);
    }),
  );
  const queryClient = new QueryClient({ defaultOptions: { queries: { gcTime: 0 } } });
  return renderHook(useHealth, {
    wrapper: ({ children }: PropsWithChildren) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    ),
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('Session protocol health gate', () => {
  it('accepts the protocol declared by the current Rust server and client', async () => {
    expect(clientProtocol).toBe(serverProtocol);
    const { result } = healthQuery(serverProtocol);
    await waitFor(() => {
      expect(result.current.isSuccess).toBe(true);
    });
    expect(result.current.data?.mismatch).toBeUndefined();
  });

  it('reports a mismatch instead of accepting a different server protocol', async () => {
    const previous = serverProtocol - 1;
    const { result } = healthQuery(previous);
    await waitFor(() => {
      expect(result.current.isSuccess).toBe(true);
    });
    expect(result.current.data?.mismatch).toBe(
      `kuku fixture uses protocol ${String(previous)}. This browser interface uses protocol ${String(serverProtocol)}.`,
    );
  });
});
