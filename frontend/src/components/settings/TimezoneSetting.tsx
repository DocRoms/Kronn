import { useEffect, useRef, useState } from 'react';
import { Clock, Save } from 'lucide-react';
import { config as configApi } from '../../lib/api';
import { COMMON_TIMEZONES } from '../../lib/watchTrigger';
import type { ToastFn } from '../../hooks/useToast';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';

interface Props {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

/** Kronn's global timezone (KT-1103): crons and date templates without their own zone. */
export function TimezoneSetting({ toast, t }: Props) {
  const [value, setValue] = useState('');
  const [effective, setEffective] = useState<string | null>(null);
  const [detected, setDetected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A slow first load never overwrites what the user typed or saved.
  const editedRef = useRef(false);
  const savedRef = useRef(false);

  useEffect(() => {
    let live = true;
    configApi.getServerConfig()
      .then(cfg => {
        if (!live || !cfg) return;
        // The machine zone never changes with a save; the rest may be stale.
        setDetected(cfg.timezone_detected);
        if (savedRef.current) return;
        if (!editedRef.current) setValue(cfg.timezone ?? '');
        setEffective(cfg.timezone_effective);
      })
      .catch(() => {});
    return () => { live = false; };
  }, []);

  const save = useAsyncGuard(async () => {
    const zone = value.trim();
    try {
      await configApi.setServerConfig({ timezone: zone });
      savedRef.current = true;
      setError(null);
      setEffective(zone || detected);
      toast(t('config.timezoneSaved'), 'success');
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  });

  return (
    <div className="mb-8" data-testid="settings-timezone">
      <div className="flex-row gap-3 mb-3">
        <Clock size={12} className="text-tertiary" />
        <label className="label" htmlFor="settings-timezone-input" style={{ marginBottom: 0 }}>{t('config.timezone')}</label>
        {effective && <span className="text-xs text-muted">{t('config.timezoneEffective', effective)}</span>}
      </div>
      <div className="flex-row gap-3">
        <input
          id="settings-timezone-input"
          className="set-domain-input"
          list="settings-timezone-zones"
          value={value}
          onChange={e => { editedRef.current = true; setValue(e.target.value); }}
          onKeyDown={e => { if (e.key === 'Enter') void save(); }}
          placeholder={detected ?? 'UTC'}
          spellCheck={false}
        />
        <datalist id="settings-timezone-zones">
          {COMMON_TIMEZONES.map(zone => <option key={zone} value={zone} />)}
        </datalist>
        <button className="set-icon-btn" aria-label={t('config.timezoneSave')} onClick={() => void save()}>
          <Save size={11} />
        </button>
      </div>
      {error && <div className="set-hint-xs text-error" role="alert">{error}</div>}
      <div className="set-hint-xs">{t('config.timezoneHint', detected ?? 'UTC')}</div>
    </div>
  );
}
