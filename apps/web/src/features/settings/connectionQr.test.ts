import { describe, expect, it } from 'vitest';

import type { ConnectionInfo } from '../../api/generated';
import { buildCredentialQrValue } from './connectionQr';

const info: ConnectionInfo = {
  display_name: 'Fixture Server',
  lan_url: 'http://phone-host:17777/',
  local_url: 'http://127.0.0.1:17777/',
  plaintext: true,
  preferred_origin: 'http://phone-host:17777/',
  server_id: 'server_fixture_1',
};

describe('buildCredentialQrValue', () => {
  it('places the credential only in the URL fragment', () => {
    const value = buildCredentialQrValue(info, 'fixture token');

    expect(value).toBe('http://phone-host:17777/#credential=fixture%20token');
    expect(value).not.toContain('?credential=');
  });
});
