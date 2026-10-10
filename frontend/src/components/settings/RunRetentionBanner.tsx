import { useEffect, useState } from 'react';
import { DatabaseZap, X } from 'lucide-react';
import { config } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import { safeGetItem, safeSetItem } from '../../lib/safeStorage';
import { settingsSectionPath } from '../../lib/routes';
import { AppLink } from '../AppLink';
import './RunRetentionBanner.css';

export const RUN_RETENTION_BANNER_KEY = 'kronn:runRetentionBannerDismissed';
/** Past this size a dismissal no longer hides the banner. */
export const RUN_RETENTION_REAPPEAR_BYTES = 2 * 1024 * 1024 * 1024;
export const RETENTION_FOCUS_TARGET = 'run-payload-retention';

function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} Ko`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} Mo`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} Go`;
}

interface Props {
  /** Known retention window; when omitted the banner reads it from the server config. */
  retentionDays?: number;
  /** A plain click on the setting link, in this tab: navigate (or, on
   *  Configuration itself, scroll) to `/config#run-payload-retention`. */
  onOpenSetting: () => void;
}

/** Shown while run payload retention is off: the database keeps every run output. */
export function RunRetentionBanner({ retentionDays, onOpenSetting }: Props) {
  const { t } = useT();
  const [fetched, setFetched] = useState<number | null>(null);
  const [bytes, setBytes] = useState<number | null>(null);
  const [dismissed, setDismissed] = useState(() => safeGetItem(RUN_RETENTION_BANNER_KEY) === '1');
  const days = retentionDays ?? fetched;
  const off = days === 0;

  useEffect(() => {
    if (retentionDays !== undefined) return;
    let live = true;
    config.getServerConfig()
      .then(cfg => { if (live) setFetched(cfg.run_payload_retention_days ?? null); })
      .catch(() => {});
    return () => { live = false; };
  }, [retentionDays]);

  // The measurement walks the b-trees, so it only runs when the banner can show.
  useEffect(() => {
    if (!off) return;
    let live = true;
    config.dbUsage()
      .then(usage => { if (live) setBytes(usage.file_bytes); })
      .catch(() => {});
    return () => { live = false; };
  }, [off]);

  const large = bytes !== null && bytes >= RUN_RETENTION_REAPPEAR_BYTES;
  if (!off || (dismissed && !large)) return null;

  const dismiss = () => {
    safeSetItem(RUN_RETENTION_BANNER_KEY, '1');
    setDismissed(true);
  };

  return (
    <section className="rr-banner" role="note" data-testid="run-retention-banner">
      <DatabaseZap size={18} aria-hidden="true" />
      <div className="rr-banner-body">
        <strong>{t('config.runRetentionBannerTitle')}</strong>
        {bytes !== null && <span>{t('config.runRetentionBannerSize', formatBytes(bytes))}</span>}
        <div className="rr-banner-actions">
          <AppLink className="rr-btn" to={settingsSectionPath(RETENTION_FOCUS_TARGET)} onNavigate={onOpenSetting}>
            {t('config.runRetentionBannerOpen')}
          </AppLink>
          <button type="button" className="rr-btn" onClick={dismiss}>
            <X size={12} aria-hidden="true" /> {t('config.runRetentionBannerDismiss')}
          </button>
        </div>
      </div>
    </section>
  );
}
