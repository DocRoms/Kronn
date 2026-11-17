// KT-1524 — the API connections whose token the provider refuses, so a revoked
// or expired token shows before (and not only after) a workflow fails on it.
// They are notices of the bell (KT-1686), not a banner.
import { useEffect } from 'react';
import { mcps as mcpsApi } from '../lib/api';
import { connectionsNeedingAttention } from '../lib/authRefusal';
import { setNotices } from '../lib/notifications';
import { pluginPath } from '../lib/routes';

/** The flag clears server-side on the next good call: re-read now and then. */
const REFRESH_MS = 120_000;

export function useAuthFailureNotices(): void {
  useEffect(() => {
    let live = true;
    const load = () => {
      Promise.resolve()
        .then(() => mcpsApi.overview())
        .then(overview => {
          if (!live) return;
          setNotices('token', connectionsNeedingAttention(overview.configs).map(config => ({
            id: config.id,
            kind: 'token_refused' as const,
            label: config.label,
            link: pluginPath(config.id),
            category: 'automations' as const,
          })));
        })
        .catch(() => {});
    };
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => { live = false; window.clearInterval(timer); };
  }, []);
}
