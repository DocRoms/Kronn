import { useRef, useState } from 'react';
import { Check, Copy, Loader2 } from 'lucide-react';
import type { ContextFile } from '../types/generated';
import { discussions } from '../lib/api';
import { browserCopyEnv, copyAsset, copyRefusal, CopyAssetError, type CopyEnv, type CopyKind, type CopyRefusal } from '../lib/copyAsset';

const REFUSAL_KEYS: Record<CopyRefusal, string> = {
  insecure: 'disc.assets.copyInsecure',
  unsupported: 'disc.assets.copyUnsupported',
  video_unsupported: 'disc.assets.copyVideoUnsupported',
  too_large: 'disc.assets.copyTooLarge',
  denied: 'disc.assets.copyDenied',
};

/** Mounted with a file key, so feedback never carries over to the next asset. */
export function CopyAssetButton({ file, kind, t, env = browserCopyEnv() }: {
  file: ContextFile;
  kind: CopyKind;
  t: (key: string, ...args: (string | number)[]) => string;
  env?: CopyEnv;
}) {
  const [state, setState] = useState<'idle' | 'copying' | 'copied'>('idle');
  const [error, setError] = useState<CopyRefusal | null>(null);
  // `disabled` only applies after a render; two clicks in one tick would both run.
  const inFlight = useRef(false);
  const unavailable = copyRefusal(kind, file.mime_type, env);
  const label = unavailable
    ? t(REFUSAL_KEYS[unavailable])
    : t(state === 'copied' ? 'disc.assets.copied' : 'disc.assets.copy');

  const copy = async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setState('copying');
    setError(null);
    try {
      await copyAsset(kind, file.mime_type, () => discussions.contextFileBlob(file.discussion_id, file.id), env);
      setState('copied');
    } catch (e) {
      setError(e instanceof CopyAssetError ? e.reason : 'denied');
      setState('idle');
    } finally {
      inFlight.current = false;
    }
  };

  return (
    <>
      <button
        type="button"
        className="disc-image-lightbox-action"
        disabled={Boolean(unavailable) || state === 'copying'}
        onClick={() => void copy()}
        aria-label={label}
        title={label}
        data-testid="attachment-copy"
        data-state={state}
      >
        {state === 'copying' ? <Loader2 size={18} /> : state === 'copied' ? <Check size={18} /> : <Copy size={18} />}
      </button>
      {error && (
        <p className="disc-image-lightbox-error" role="alert" data-testid="attachment-copy-error">
          {t(REFUSAL_KEYS[error])}
        </p>
      )}
    </>
  );
}
