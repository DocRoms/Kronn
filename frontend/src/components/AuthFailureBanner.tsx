// KT-1524 — app-wide notice listing the API connections whose token the
// provider refuses, so a revoked or expired token shows before (and not only
// after) a workflow fails on it.
import { useEffect, useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { mcps as mcpsApi } from '../lib/api';
import { connectionsNeedingAttention } from '../lib/authRefusal';
import { pluginPath } from '../lib/routes';
import type { McpConfigDisplay } from '../types/generated';
import { AppLink } from './AppLink';

/** The flag clears server-side on the next good call: re-read now and then. */
const REFRESH_MS = 120_000;

export function AuthFailureBanner() {
  const { t } = useT();
  const [refused, setRefused] = useState<McpConfigDisplay[]>([]);

  useEffect(() => {
    let live = true;
    const load = () => {
      Promise.resolve()
        .then(() => mcpsApi.overview())
        .then(overview => { if (live) setRefused(connectionsNeedingAttention(overview.configs)); })
        .catch(() => {});
    };
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => { live = false; window.clearInterval(timer); };
  }, []);

  if (refused.length === 0) return null;
  return (
    <div className="set-expose-warn" role="alert" data-testid="auth-failure-banner">
      <AlertTriangle size={13} />
      <strong>{t('authFailure.banner.title')}</strong>
      <span>{t('authFailure.banner.body', refused.map(config => config.label).join(', '))}</span>
      {refused.map(config => (
        <AppLink
          key={config.id}
          to={pluginPath(config.id)}
          className="kr-inline-link"
          data-testid={`auth-failure-fix-${config.id}`}
        >
          {t('authFailure.banner.fix', config.label)}
        </AppLink>
      ))}
    </div>
  );
}
