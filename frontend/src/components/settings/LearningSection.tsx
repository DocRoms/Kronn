import { useEffect, useState } from 'react';
import { GraduationCap } from 'lucide-react';
import { config as configApi } from '../../lib/api';
import type { ToastFn } from '../../hooks/useToast';
import { userError } from '../../lib/userError';
import '../../pages/SettingsPage.css';

interface LearningSectionProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

/**
 * Mode Mentor — master on/off switch for the whole guided-learning feature
 * (Mentor + Onboarding). Backed by `ServerConfig.mentor_enabled` and read on
 * mount. Toggling fires `kronn:mentor-enabled-changed` so the Dashboard nav
 * shows/hides the "Apprendre" tab live without a reload.
 */
export function LearningSection({ toast, t }: LearningSectionProps) {
  const [enabled, setEnabled] = useState(false);

  useEffect(() => {
    let alive = true;
    configApi.getServerConfig()
      .then(cfg => { if (alive) setEnabled(cfg.mentor_enabled); })
      .catch(() => {});
    return () => { alive = false; };
  }, []);

  const onToggle = async (next: boolean) => {
    setEnabled(next);
    try {
      await configApi.setServerConfig({ mentor_enabled: next });
      window.dispatchEvent(new CustomEvent('kronn:mentor-enabled-changed'));
      toast(next ? t('settings.learning.enabledToast') : t('settings.learning.disabledToast'), 'success');
    } catch (err) {
      setEnabled(!next);
      toast(t('common.actionFailed', userError(err)), 'error');
    }
  };

  return (
    <div id="settings-learning" className="set-card">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <GraduationCap size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.learning.title')}</span>
          <span className="set-beta-badge" title={t('settings.betaBadgeHint')}>{t('settings.betaBadge')}</span>
        </div>
        <p className="set-hint">{t('settings.learning.intro')}</p>

        <div className="flex-row gap-4 mb-3" style={{ alignItems: 'center', marginTop: 8 }}>
          <span className="label" style={{ marginBottom: 0 }}>{t('settings.learning.toggleLabel')}</span>
          <label className="flex-row gap-2" style={{ cursor: 'pointer', marginLeft: 'auto', alignItems: 'center' }}>
            <input
              type="checkbox"
              checked={enabled}
              data-testid="settings-mentor-enabled"
              onChange={e => onToggle(e.target.checked)}
            />
            <span className="text-sm">{enabled ? t('common.on') : t('common.off')}</span>
          </label>
        </div>
        <div className="set-hint-xs">{t('settings.learning.hint')}</div>
      </div>
    </div>
  );
}
