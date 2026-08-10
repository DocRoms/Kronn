// 0.10.0 — Continual Learning master toggle (beta, default OFF). Flipping it ON
// lets agents propose durable learnings (gated by evidence + human validation)
// and wires the `docs/learnings.md` pointer into audited projects. OFF by
// default because the feature writes into injected truth files — opt-in.

import { useEffect, useState } from 'react';
import { AlertTriangle, BookOpen, Lightbulb } from 'lucide-react';
import { config as configApi } from '../../lib/api';
import type { ToastFn } from '../../hooks/useToast';
import '../../pages/SettingsPage.css';

interface ContinualLearningSectionProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ContinualLearningSection({ toast, t }: ContinualLearningSectionProps) {
  const [enabled, setEnabled] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    let alive = true;
    configApi
      .getContinualLearningEnabled()
      .then((v) => {
        if (alive) {
          setEnabled(!!v);
          setLoaded(true);
        }
      })
      .catch(() => {
        if (alive) setLoaded(true);
      });
    return () => {
      alive = false;
    };
  }, []);

  const onToggle = (next: boolean) => {
    const prev = enabled;
    setEnabled(next);
    configApi
      .saveContinualLearningEnabled(next)
      .then(() => toast(t('settings.clSaved'), 'success'))
      .catch(() => {
        setEnabled(prev);
        toast(t('settings.clSaveError'), 'error');
      });
  };

  return (
    <div id="settings-continual-learning" className="set-card set-beta-feature-card" data-feature="learning">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <BookOpen size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.clTitle')}</span>
          <span className="set-beta-badge" title={t('settings.betaBadgeHint')}>
            {t('settings.betaBadge')}
          </span>
        </div>
        <p className="set-hint">{t('settings.clDesc')}</p>
        <div className="set-beta-feature-stack">
          <section className="set-beta-feature-panel" data-kind="control" data-enabled={enabled}>
            <div className="set-beta-feature-panel-head">
              <div className="set-beta-feature-panel-icon"><Lightbulb size={16} /></div>
              <div className="flex-1">
                <h3>{t('settings.clToggleLabel')}</h3>
                <p>{t('settings.clHint')}</p>
              </div>
              <button
                type="button"
                className="set-beta-feature-switch"
                role="switch"
                aria-checked={enabled}
                aria-label={t('settings.clToggleLabel')}
                disabled={!loaded}
                onClick={() => onToggle(!enabled)}
              >
                <span className="set-beta-feature-switch-track" aria-hidden="true">
                  <span />
                </span>
                <strong>{enabled ? t('common.on') : t('common.off')}</strong>
              </button>
            </div>
          </section>

          <aside
            className="set-cl-risk-warning"
            aria-label={t('settings.clRiskTitle')}
            data-testid="continual-learning-risk-warning"
          >
            <div className="set-cl-risk-warning-title">
              <AlertTriangle size={16} aria-hidden="true" />
              <strong>{t('settings.clRiskTitle')}</strong>
            </div>
            <p>{t('settings.clRiskBody')}</p>
          </aside>
        </div>
      </div>
    </div>
  );
}
