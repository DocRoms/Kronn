import { useEffect, useState } from 'react';
import { config as configApi } from '../lib/api';
import { userError } from '../lib/userError';

interface P2pOffNoticeProps {
  contactCount: number;
  t: (key: string, ...args: (string | number)[]) => string;
}

/** Explains why contacts stay offline once P2P is off (KT-1033), with a one-click enable. */
export function P2pOffNotice({ contactCount, t }: P2pOffNoticeProps) {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    configApi.getServerConfig().then(config => setEnabled(config?.p2p_enabled ?? null)).catch(() => {});
  }, []);

  if (contactCount === 0 || enabled !== false) return null;
  return (
    <div className="disc-p2p-off-notice" role="note" data-testid="p2p-off-notice">
      <p>{t('contacts.p2pOff')}</p>
      <p className="disc-p2p-off-warning">{t('settings.p2pWarning')}</p>
      <button
        type="button"
        className="disc-contact-add-submit"
        onClick={() => {
          configApi.setServerConfig({ p2p_enabled: true })
            .then(() => { setEnabled(true); setError(null); })
            .catch(err => setError(userError(err)));
        }}
      >
        {t('contacts.p2pEnable')}
      </button>
      {error && <p role="alert">{error}</p>}
    </div>
  );
}
