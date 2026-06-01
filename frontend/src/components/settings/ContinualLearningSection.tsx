// 0.9.0 — Continual Learning master toggle (beta, default OFF). Flipping it ON
// lets agents propose durable learnings (gated by evidence + human validation)
// and wires the `docs/learnings.md` pointer into audited projects. OFF by
// default because the feature writes into injected truth files — opt-in.

import { useEffect, useState } from 'react';
import { BookOpen } from 'lucide-react';
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
    <div id="settings-continual-learning" className="set-card">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <BookOpen size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.clTitle')}</span>
          <span className="set-beta-badge" title={t('settings.betaBadgeHint')}>
            {t('settings.betaBadge')}
          </span>
        </div>
        <p className="set-hint">{t('settings.clDesc')}</p>
        <div className="flex-row gap-4 mb-3" style={{ alignItems: 'center' }}>
          <span className="label" style={{ marginBottom: 0 }}>
            {t('settings.clToggleLabel')}
          </span>
          <label
            className="flex-row gap-2"
            style={{ cursor: 'pointer', marginLeft: 'auto', alignItems: 'center' }}
          >
            <input
              type="checkbox"
              checked={enabled}
              disabled={!loaded}
              onChange={(e) => onToggle(e.target.checked)}
            />
            <span className="text-sm">{enabled ? t('common.on') : t('common.off')}</span>
          </label>
        </div>
        <p className="set-hint">{t('settings.clHint')}</p>
      </div>
    </div>
  );
}
