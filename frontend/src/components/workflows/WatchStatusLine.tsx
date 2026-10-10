import { AlertTriangle, Eye } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import type { WatchStatus } from '../../types/generated';
import './WatchStatusLine.css';

/** Poll history of a Watch workflow on its card: polls create no run, so this is their only trace. */
export function WatchStatusLine({ status }: { status: WatchStatus }) {
  const { t } = useT();
  return (
    <div className="wf-watch-status" data-testid="watch-status" data-failing={status.failing}>
      <Eye size={11} aria-hidden />
      <span>
        {status.last_poll_at
          ? t('wf.watch.lastPoll', new Date(status.last_poll_at).toLocaleString())
          : t('wf.watch.neverPolled')}
      </span>
      {status.last_result && (
        <span className="wf-watch-status-result" data-result={status.last_result}>
          {t(`wf.watch.result.${status.last_result}`)}
          {status.last_http_status != null && ` (${status.last_http_status})`}
        </span>
      )}
      <span className="wf-watch-status-counters" title={t('wf.watch.countersTip')}>
        {t('wf.watch.counters', String(status.unchanged_count), String(status.changed_count), String(status.error_count))}
      </span>
      {status.failing && (
        <span className="wf-watch-failing-badge" title={status.last_error ?? undefined} role="alert">
          <AlertTriangle size={10} />
          {t('wf.watch.failing', String(status.consecutive_failures))}
        </span>
      )}
    </div>
  );
}
