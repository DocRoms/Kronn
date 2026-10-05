import { useState } from 'react';
import { ShieldAlert, X } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { AGENT_LABELS } from '../../lib/constants';
import { safeGetItem, safeSetItem } from '../../lib/safeStorage';
import type { AgentEffectiveAccess } from '../../types/generated';
import './AgentFullAccessNotice.css';

export const FULL_ACCESS_NOTICE_KEY = 'kronn:fullAccessNoticeDismissed';

/** Shown once to installs that never went through the setup wizard's Access step:
 *  which agents really run with full access, what that means, and where to change it. */
export function AgentFullAccessNotice({ rows }: { rows: AgentEffectiveAccess[] }) {
  const { t } = useT();
  const [dismissed, setDismissed] = useState(() => safeGetItem(FULL_ACCESS_NOTICE_KEY) === '1');
  const full = rows.filter(row => row.full_access);
  if (dismissed || full.length === 0) return null;

  const dismiss = () => {
    safeSetItem(FULL_ACCESS_NOTICE_KEY, '1');
    setDismissed(true);
  };
  const showSwitches = () => {
    document.querySelector('.set-agent-access-row')?.scrollIntoView?.({ behavior: 'smooth', block: 'center' });
  };

  return (
    <section className="fa-notice" role="note" aria-labelledby="fa-notice-title" data-testid="full-access-notice">
      <ShieldAlert size={18} aria-hidden="true" />
      <div className="fa-notice-body">
        <strong id="fa-notice-title">{t('config.fullAccessNoticeTitle')}</strong>
        <p>{t('config.fullAccessRisk')}</p>
        <ul>
          {full.map(row => (
            <li key={row.agent}>
              {AGENT_LABELS[row.agent] ?? row.agent}
              {' — '}
              {row.reason === 'forced_in_container'
                ? t('config.fullAccessForcedInContainer')
                : t('config.fullAccessNoticeBySetting')}
            </li>
          ))}
        </ul>
        <div className="fa-notice-actions">
          <button type="button" className="fa-btn" onClick={showSwitches}>
            {t('config.fullAccessNoticeReview')}
          </button>
          <button type="button" className="fa-btn" onClick={dismiss}>
            <X size={12} aria-hidden="true" /> {t('config.fullAccessNoticeDismiss')}
          </button>
        </div>
      </div>
    </section>
  );
}
