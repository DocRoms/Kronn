// KT-1492 — Settings › Trust levels: the instance's default trust level, and the
// journal of what a level granted on its own.
import { useCallback, useEffect, useState } from 'react';
import { ShieldAlert } from 'lucide-react';
import { trust as trustApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import type { ToastFn } from '../../hooks/useToast';
import type { TrustAuditEntry, TrustOverview } from '../../types/generated';
import { TRUST_SETTINGS_SECTION, TrustBadge, TrustLevelControl } from '../TrustLevel';
import { TrustProofSection } from './TrustProofSection';

const CHANGED = 'kronn:trust-changed';

/** KT-1495 — whether Kronn's vault (data directory) is closed to the agent
 *  processes, whatever the trust level: active, turned off, or unavailable here. */
function VaultStatus({ sandbox }: { sandbox: TrustOverview['sandbox'] | null }) {
  const { t } = useT();
  if (!sandbox) return null;
  const state = sandbox.protected ? 'active' : sandbox.protect_vault ? 'unavailable' : 'off';
  return (
    <p className="trust-vault-note" role="status" data-testid="trust-vault-status" data-state={state}>
      {state === 'unavailable' ? t('trust.vault.unavailable', sandbox.reason ?? '') : t(`trust.vault.${state}`)}
    </p>
  );
}

export function TrustSection({ toast }: { toast?: ToastFn }) {
  const { t } = useT();
  const [audit, setAudit] = useState<TrustAuditEntry[]>([]);
  const [sandbox, setSandbox] = useState<TrustOverview['sandbox'] | null>(null);
  const load = useCallback(() => {
    Promise.resolve()
      .then(() => trustApi.audit(50))
      .then(entries => setAudit(entries ?? []))
      .catch(() => setAudit([]));
  }, []);
  useEffect(() => {
    Promise.resolve()
      .then(() => trustApi.overview())
      .then(loaded => setSandbox(loaded?.sandbox ?? null))
      .catch(() => setSandbox(null));
  }, []);
  useEffect(() => {
    load();
    window.addEventListener(CHANGED, load);
    return () => window.removeEventListener(CHANGED, load);
  }, [load]);

  return (
    <div id={TRUST_SETTINGS_SECTION} className="set-card" data-testid="settings-trust">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <ShieldAlert size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('trust.section.title')}</span>
          <TrustBadge target={{ scope: 'instance' }} toast={toast} />
        </div>
        <p className="set-meta">{t('trust.section.hint')}</p>
        <VaultStatus sandbox={sandbox} />
        <TrustLevelControl target={{ scope: 'instance' }} toast={toast} />
        <TrustProofSection toast={toast} />
        <h4 className="mt-8">{t('trust.audit.title')}</h4>
        {audit.length === 0 ? (
          <p className="set-meta" data-testid="trust-audit-empty">{t('trust.audit.empty')}</p>
        ) : (
          <ul className="trust-audit" data-testid="trust-audit">
            {audit.map(entry => (
              <li key={entry.id}>
                <time dateTime={entry.at}>{new Date(entry.at).toLocaleString()}</time>
                {entry.event === 'withdrawn' && <em>{t('trust.audit.withdrawn')} · </em>}
                {entry.event === 'proof' && <em>{t('trust.audit.proof')} · </em>}
                {entry.event === 'proof_refused' && <em>{t('trust.audit.proofRefused')} · </em>}
                <strong>{entry.subject}</strong> — {entry.detail}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
