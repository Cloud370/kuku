import { describe, expect, it, vi } from 'vitest';

import { WebApiError } from '../../api/client';
import settingsSnapshotJson from '../../api/generated/fixtures/settings_snapshot.json';
import type { SettingsPatch, SettingsSnapshot } from '../../api/generated';
import { submitSettingsPatch } from './settingsActions';

const settings = structuredClone(settingsSnapshotJson) as SettingsSnapshot;
const patch: SettingsPatch = {
  default_tier: settings.default_tier,
  default_workspace_id: null,
  max_concurrent_runs: 8,
};

function apiError(status: number): WebApiError {
  return new WebApiError(status, {
    api_version: 1,
    code: status === 409 ? 'stale_server_revision' : 'invalid_request',
    details: null,
    message: 'fixture error',
    trace_id: 'trace-settings',
  });
}

describe('submitSettingsPatch', () => {
  it('refetches current server Settings after a revision conflict', async () => {
    const refreshed = { ...settings, server_revision: 'revision-refreshed' };
    const get = vi.fn().mockResolvedValue(refreshed);
    const result = await submitSettingsPatch(
      { get, update: vi.fn().mockRejectedValue(apiError(409)) },
      settings,
      patch,
    );

    expect(result).toEqual({ kind: 'reconciled', snapshot: refreshed });
    expect(get).toHaveBeenCalledTimes(1);
  });

  it('does not reconcile a definitive validation failure', async () => {
    const get = vi.fn();
    await expect(
      submitSettingsPatch(
        { get, update: vi.fn().mockRejectedValue(apiError(400)) },
        settings,
        patch,
      ),
    ).rejects.toMatchObject({ status: 400 });
    expect(get).not.toHaveBeenCalled();
  });
});
