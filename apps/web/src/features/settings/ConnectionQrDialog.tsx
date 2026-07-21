import QRCode from 'qrcode';
import { X } from 'lucide-react';
import { useEffect, useState } from 'react';

import { webApi } from '../../api/client';
import type { ConnectionInfo } from '../../api/generated';
import { buildCredentialQrValue } from './connectionQr';

export interface CredentialQrEncoder {
  encode(value: string): Promise<string>;
}

interface CredentialReader {
  current(): string | null;
}

const defaultEncoder: CredentialQrEncoder = {
  encode: (value) => QRCode.toDataURL(value, { errorCorrectionLevel: 'M', margin: 1, width: 320 }),
};

interface ConnectionQrDialogProps {
  credentials?: CredentialReader;
  encoder?: CredentialQrEncoder;
  info: ConnectionInfo;
  onClose?: () => void;
  open: boolean;
}

type QrState =
  | { kind: 'loading' }
  | { kind: 'ready'; dataUrl: string }
  | { kind: 'unavailable' }
  | { kind: 'error' };

export function ConnectionQrDialog({
  credentials = webApi.credentials,
  encoder = defaultEncoder,
  info,
  onClose,
  open,
}: ConnectionQrDialogProps) {
  const [state, setState] = useState<QrState>({ kind: 'loading' });

  useEffect(() => {
    if (!open) {
      setState({ kind: 'loading' });
      return;
    }
    const credential = credentials.current();
    if (credential === null) {
      setState({ kind: 'unavailable' });
      return;
    }
    let current = true;
    setState({ kind: 'loading' });
    void encoder.encode(buildCredentialQrValue(info, credential)).then(
      (dataUrl) => {
        if (current) setState({ dataUrl, kind: 'ready' });
      },
      () => {
        if (current) setState({ kind: 'error' });
      },
    );
    return () => {
      current = false;
    };
  }, [credentials, encoder, info, open]);

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/60 p-4">
      <section
        aria-label="Connection QR"
        aria-modal="true"
        className="w-full max-w-sm border border-[var(--color-border)] bg-[var(--color-surface-raised)] p-4 shadow-[var(--shadow-elevated)]"
        role="dialog"
      >
        <div className="flex items-center justify-between gap-3">
          <h2 className="text-sm font-semibold">Connection QR</h2>
          <button
            aria-label="Close Connection QR"
            className="inline-flex size-8 items-center justify-center focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
            onClick={onClose}
            type="button"
          >
            <X aria-hidden="true" size={16} />
          </button>
        </div>
        <div className="mt-4 grid min-h-72 place-items-center text-center text-sm">
          {state.kind === 'loading' ? <p role="status">Preparing QR</p> : null}
          {state.kind === 'ready' ? (
            <img alt="Connection QR code" className="size-72" src={state.dataUrl} />
          ) : null}
          {state.kind === 'unavailable' ? (
            <div>
              <p>Connection QR unavailable on this browser</p>
              <p className="mt-2 break-all text-xs text-[var(--color-text-secondary)]">
                {info.lan_url ?? info.preferred_origin}
              </p>
            </div>
          ) : null}
          {state.kind === 'error' ? <p role="alert">Connection QR could not be created.</p> : null}
        </div>
      </section>
    </div>
  );
}
