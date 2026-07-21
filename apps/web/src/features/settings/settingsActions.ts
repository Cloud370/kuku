import { WebApiError } from '../../api/client';
import type { SettingsPatch, SettingsSnapshot, UpdateSettingsRequest } from '../../api/generated';

export interface SettingsOperations {
  get: () => Promise<SettingsSnapshot>;
  update: (body: UpdateSettingsRequest) => Promise<SettingsSnapshot>;
}

export type SettingsUpdateResult =
  | { kind: 'updated'; snapshot: SettingsSnapshot }
  | { kind: 'reconciled'; snapshot: SettingsSnapshot };

export async function submitSettingsPatch(
  operations: SettingsOperations,
  current: SettingsSnapshot,
  patch: SettingsPatch,
): Promise<SettingsUpdateResult> {
  try {
    const snapshot = await operations.update({
      expected_revision: current.server_revision,
      patch,
    });
    return { kind: 'updated', snapshot };
  } catch (error) {
    if (error instanceof WebApiError && error.status !== 409) throw error;
    const snapshot = await operations.get();
    return { kind: 'reconciled', snapshot };
  }
}
