import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';

import { webApi } from '../../api/client';
import type {
  PlatformCatalog,
  PlatformStatus,
  SettingsSnapshot,
  WorkspacePage,
} from '../../api/generated';
import { ResponsiveFeatureSurface } from '../experience/ResponsiveFeatureSurface';
import { ConnectionQrDialog } from './ConnectionQrDialog';
import { SettingsSections, type SettingsDraft } from './SettingsSections';
import { submitSettingsPatch, type SettingsOperations } from './settingsActions';

export interface SettingsRouteApi {
  catalog: { platform: () => Promise<PlatformCatalog> };
  credentials: { current: () => string | null };
  platform: { status: () => Promise<PlatformStatus> };
  settings: SettingsOperations;
  workspaces: { list: () => Promise<WorkspacePage> };
}

export type SettingsInitialDialog = 'connection_qr';

interface SettingsRouteProps {
  api?: SettingsRouteApi;
  initialDialog?: SettingsInitialDialog | null;
  onOpenGuide?: () => void;
}

const settingsKey = ['settings'] as const;
const platformCatalogKey = ['platform-catalog'] as const;

export function SettingsRoute({
  api = webApi,
  initialDialog = null,
  onOpenGuide,
}: SettingsRouteProps) {
  const queryClient = useQueryClient();
  const settings = useQuery({ queryKey: settingsKey, queryFn: () => api.settings.get() });
  const catalog = useQuery({
    queryKey: platformCatalogKey,
    queryFn: () => api.catalog.platform(),
  });
  const status = useQuery({ queryKey: ['platform-status'], queryFn: () => api.platform.status() });
  const workspaces = useQuery({ queryKey: ['workspaces'], queryFn: () => api.workspaces.list() });
  const [draft, setDraft] = useState<SettingsDraft | null>(null);
  const [qrOpen, setQrOpen] = useState(false);
  const initialDialogOpened = useRef(false);
  const [notice, setNotice] = useState<{
    message: string;
    role: 'alert' | 'status';
  } | null>(null);

  useEffect(() => {
    if (settings.data !== undefined && draft === null) setDraft(draftFrom(settings.data));
  }, [draft, settings.data]);

  const save = useMutation({
    mutationFn: async (currentDraft: SettingsDraft) => {
      if (settings.data === undefined) throw new Error('Settings are not loaded');
      return submitSettingsPatch(api.settings, settings.data, {
        default_tier: currentDraft.defaultTier,
        default_workspace_id:
          currentDraft.defaultWorkspaceId.length === 0 ? null : currentDraft.defaultWorkspaceId,
        max_concurrent_runs: Number(currentDraft.maxConcurrentRuns),
        discovery: { auto_discover: currentDraft.autoDiscover },
      });
    },
    onSuccess: (result) => {
      queryClient.setQueryData(settingsKey, result.snapshot);
      if (result.kind === 'updated') {
        setDraft(draftFrom(result.snapshot));
        setNotice({ message: 'Settings saved', role: 'status' });
      } else {
        setNotice({
          message: 'Settings reconciled with current server state. Review and save again.',
          role: 'alert',
        });
      }
    },
  });

  const loading =
    settings.isPending || catalog.isPending || status.isPending || workspaces.isPending;
  const error = settings.error ?? catalog.error ?? status.error ?? workspaces.error;
  const ready =
    !loading &&
    error === null &&
    draft !== null &&
    catalog.data !== undefined &&
    status.data !== undefined &&
    workspaces.data !== undefined;

  useEffect(() => {
    if (ready && initialDialog === 'connection_qr' && !initialDialogOpened.current) {
      initialDialogOpened.current = true;
      setQrOpen(true);
    }
  }, [initialDialog, ready]);

  const transition = save.isPending
    ? { kind: 'saving' as const, message: 'Saving Settings' }
    : undefined;
  const maxConcurrentRuns = Number(draft?.maxConcurrentRuns ?? '');
  const canSave = draft !== null && Number.isInteger(maxConcurrentRuns) && maxConcurrentRuns > 0;

  return (
    <ResponsiveFeatureSurface
      kind="settings"
      mode="full"
      status={transition}
      toolbar={
        <div className="flex w-full items-center justify-between gap-3">
          <h1 className="text-sm font-semibold">Settings</h1>
          <button
            className="bg-[var(--color-accent)] px-3 py-2 text-xs font-medium text-[var(--color-accent-contrast)] disabled:opacity-40"
            disabled={!canSave || save.isPending}
            onClick={() => {
              if (draft !== null) save.mutate(draft);
            }}
            type="button"
          >
            Save Settings
          </button>
        </div>
      }
    >
      {loading ? <p role="status">Loading Settings</p> : null}
      {error !== null ? (
        <div className="grid justify-items-start gap-3" role="alert">
          <p>Settings could not be loaded.</p>
          <button
            className="border border-[var(--color-border)] px-3 py-2 text-xs font-medium"
            onClick={() => {
              void Promise.all([
                settings.refetch(),
                catalog.refetch(),
                status.refetch(),
                workspaces.refetch(),
              ]);
            }}
            type="button"
          >
            Retry Settings
          </button>
        </div>
      ) : null}
      {save.isError ? <p role="alert">Settings could not be saved.</p> : null}
      {notice === null ? null : (
        <p className="mb-4 text-sm" role={notice.role}>
          {notice.message}
        </p>
      )}
      {ready ? (
        <SettingsSections
          catalog={catalog.data}
          connection={status.data.connection}
          draft={draft}
          onChange={setDraft}
          onOpenGuide={onOpenGuide}
          onOpenQr={() => {
            setQrOpen(true);
          }}
          workspaces={workspaces.data.items}
        />
      ) : null}
      {status.data === undefined ? null : (
        <ConnectionQrDialog
          credentials={api.credentials}
          info={status.data.connection}
          onClose={() => {
            setQrOpen(false);
          }}
          open={qrOpen}
        />
      )}
    </ResponsiveFeatureSurface>
  );
}

function draftFrom(snapshot: SettingsSnapshot): SettingsDraft {
  return {
    defaultTier: snapshot.default_tier,
    defaultWorkspaceId: snapshot.default_workspace_id ?? '',
    maxConcurrentRuns: String(snapshot.max_concurrent_runs),
    autoDiscover: snapshot.discovery.auto_discover,
  };
}
