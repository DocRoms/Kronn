import { useState } from 'react';
import { ShieldAlert } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import type { AutoDisabledWorkflow, AutoDisableReason } from '../../types/generated';
import './AutoDisabledReview.css';

/** i18n key of each reason Kronn records when it disables a workflow itself. */
export const AUTO_DISABLE_REASON_KEY: Record<AutoDisableReason, string> = {
  agent_edit: 'wf.autoDisabled.reason.agent_edit',
  dependency_edited_by_agent: 'wf.autoDisabled.reason.dependency_edited_by_agent',
  created_by_agent: 'wf.autoDisabled.reason.created_by_agent',
  imported: 'wf.autoDisabled.reason.imported',
};

/** A Cron or Tracker trigger runs on its own once enabled: always confirmed. */
function isScheduled(item: AutoDisabledWorkflow): boolean {
  return item.trigger.type === 'Cron' || item.trigger.type === 'Tracker';
}

interface Props {
  items: AutoDisabledWorkflow[];
  onReenable: (ids: string[]) => Promise<void>;
  onOpen: (id: string) => void;
}

/**
 * KT-1037 — workflows Kronn disabled on its own (an agent's edit or create,
 * an agent's change to a Quick API/Prompt they use, an import), listed for a
 * human to review and enable again. Hidden when there is none.
 */
export function AutoDisabledReview({ items, onReenable, onOpen }: Props) {
  const { t } = useT();
  const [open, setOpen] = useState(false);

  const reenableOne = useAsyncGuard(async (item: AutoDisabledWorkflow) => {
    if (isScheduled(item) && !window.confirm(t('wf.autoDisabled.confirmScheduled', item.name))) return;
    await onReenable([item.id]);
  });
  const reenableAll = useAsyncGuard(async () => {
    const scheduled = items.filter(isScheduled).length;
    const message = scheduled > 0
      ? t('wf.autoDisabled.confirmAllScheduled', items.length, scheduled)
      : t('wf.autoDisabled.confirmAll', items.length);
    if (!window.confirm(message)) return;
    await onReenable(items.map(item => item.id));
  });

  if (items.length === 0) return null;

  return (
    <div className="wf-auto-disabled" role="region" aria-label={t('wf.autoDisabled.title')}>
      <div className="wf-auto-disabled-banner" data-testid="auto-disabled-banner">
        <ShieldAlert size={14} />
        <span className="flex-1">{t('wf.autoDisabled.banner', items.length)}</span>
        <button
          type="button"
          className="wf-small-btn"
          aria-expanded={open}
          onClick={() => setOpen(value => !value)}
        >
          {t('wf.autoDisabled.details')}
        </button>
      </div>
      {open && (
        <div className="wf-auto-disabled-panel" data-testid="auto-disabled-panel">
          <ul className="wf-auto-disabled-list">
            {items.map(item => (
              <li key={item.id} className="wf-auto-disabled-row" data-testid="auto-disabled-row">
                <div className="wf-auto-disabled-main">
                  <button type="button" className="wf-auto-disabled-name" onClick={() => onOpen(item.id)}>
                    {item.name}
                  </button>
                  <span className="wf-auto-disabled-reason">{t(AUTO_DISABLE_REASON_KEY[item.reason])}</span>
                  <span className="wf-auto-disabled-summary">{item.summary}</span>
                  <span className="wf-auto-disabled-meta">
                    {t('wf.autoDisabled.byWhen', item.disabled_by, new Date(item.disabled_at).toLocaleString())}
                  </span>
                </div>
                <button type="button" className="wf-small-btn" onClick={() => reenableOne(item)}>
                  {t('wf.autoDisabled.reenable')}
                </button>
              </li>
            ))}
          </ul>
          <div className="wf-auto-disabled-actions">
            <button type="button" className="wf-small-btn wf-small-btn-accent" onClick={() => reenableAll()}>
              {t('wf.autoDisabled.reenableAll')}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
