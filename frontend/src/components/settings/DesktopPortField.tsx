import { useEffect, useState } from 'react';
import { config as configApi, type DesktopPortInfo } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import { userError } from '../../lib/userError';
import type { ToastFn } from '../../hooks/useToast';

interface Props {
  toast: ToastFn;
  onRestart: () => void;
}

/** Desktop only: the loopback port Kronn binds at launch. Saved to the data
 *  directory and read at the next start, so a pending change offers a restart. */
export function DesktopPortField({ toast, onRestart }: Props) {
  const { t } = useT();
  const [info, setInfo] = useState<DesktopPortInfo | null>(null);
  const [value, setValue] = useState('');
  const [pending, setPending] = useState(false);

  useEffect(() => {
    configApi.getDesktopPort()
      .then(next => { setInfo(next); setValue(String(next.saved ?? next.current)); })
      .catch(() => {});
  }, []);

  if (!info) return null;

  const port = /^\d+$/.test(value.trim()) ? Number(value.trim()) : NaN;
  const valid = Number.isInteger(port) && port >= info.min && port <= info.max;
  const unchanged = valid && port === (info.saved ?? info.current);

  const save = async () => {
    try {
      const next = await configApi.setDesktopPort(port);
      setInfo(next);
      setPending(next.saved !== null && next.saved !== next.current);
    } catch (error) {
      toast(t('common.actionFailed', userError(error)), 'error');
    }
  };

  return (
    <div className="set-desktop-port" data-testid="desktop-port-field">
      <label className="set-form-label" htmlFor="desktop-port-input">{t('settings.desktopPort')}</label>
      <div className="flex-row gap-4">
        <input
          id="desktop-port-input"
          className="set-input"
          inputMode="numeric"
          value={value}
          aria-invalid={!valid}
          onChange={event => setValue(event.target.value)}
        />
        <button type="button" className="btn btn-ghost" disabled={!valid || unchanged} onClick={save}>
          {t('settings.desktopPortSave')}
        </button>
      </div>
      <small className={valid ? undefined : 'text-error'}>
        {valid ? t('settings.desktopPortHint', info.current) : t('settings.desktopPortInvalid', info.min, info.max)}
      </small>
      {pending && (
        <div className="set-expose-restart" role="status">
          <span>{t('settings.desktopPortRestart')}</span>
          <button type="button" className="btn btn-ghost" onClick={onRestart}>
            {t('settings.desktopPortRestartBtn')}
          </button>
        </div>
      )}
    </div>
  );
}
