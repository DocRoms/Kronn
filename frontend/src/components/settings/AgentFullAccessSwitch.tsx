import { useId, useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import './AgentFullAccessSwitch.css';

interface Props {
  agentName: string;
  checked: boolean;
  /** Applies the new value; a rejection is the caller's to report. */
  onChange: (next: boolean) => Promise<void>;
  testId?: string;
  /** The setting is overridden at launch (see `config.fullAccessForcedInContainer`). */
  locked?: boolean;
}

/** The one full-access control: card, settings panel and setup wizard all use it. */
export function AgentFullAccessSwitch({ agentName, checked, onChange, testId, locked = false }: Props) {
  const { t } = useT();
  const [confirming, setConfirming] = useState(false);
  const titleId = useId();
  const apply = useAsyncGuard(async (next: boolean) => {
    setConfirming(false);
    await onChange(next);
  });

  return (
    <>
      <button
        type="button"
        role="switch"
        aria-checked={locked || checked}
        disabled={locked}
        aria-label={t('config.fullAccessSwitchAria', agentName)}
        className="fa-switch"
        data-testid={testId}
        // Enabling is the risky direction and asks first; turning it off never does.
        onClick={() => (checked ? void apply(false) : setConfirming(true))}
      >
        <span className="fa-track" data-on={locked || checked} aria-hidden="true">
          <span className="fa-thumb" data-on={locked || checked} />
        </span>
        <span className="fa-state" data-on={locked || checked}>
          {locked
            ? t('config.fullAccessForcedInContainer')
            : checked ? t('config.fullAccessStateOn') : t('config.fullAccessStateOff')}
        </span>
      </button>
      {confirming && (
        <div className="fa-backdrop" role="presentation" onClick={() => setConfirming(false)}>
          <div
            className="fa-dialog"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby={titleId}
            onClick={e => e.stopPropagation()}
            onKeyDown={e => { if (e.key === 'Escape') setConfirming(false); }}
          >
            <h3 id={titleId} className="fa-dialog-title">
              <AlertTriangle size={16} aria-hidden="true" />
              {t('config.fullAccessConfirmTitle', agentName)}
            </h3>
            <p>{t('config.fullAccessRisk')}</p>
            <div className="fa-dialog-actions">
              <button type="button" className="fa-btn" autoFocus onClick={() => setConfirming(false)}>
                {t('common.cancel')}
              </button>
              <button type="button" className="fa-btn fa-btn-danger" onClick={() => void apply(true)}>
                {t('config.fullAccessConfirmEnable')}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
