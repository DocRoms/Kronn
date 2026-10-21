import { useT } from '../lib/I18nContext';
import type { TdStateCounts } from '../types/generated';

const SEGMENTS: Array<keyof TdStateCounts> = ['confirmed', 'accepted_decision', 'deferred', 'rejected'];

/** Settled / kept / deferred / rejected share of the sheets, plus the remaining count. */
export function TdProgressBar({ counts, total }: { counts: TdStateCounts; total: number }) {
  const { t } = useT();
  return (
    <div className="td-progress">
      <div className="td-progress-bar" role="img" aria-label={t('techDebt.remaining', counts.pending + counts.deferred)}>
        {total > 0 && SEGMENTS.map(key => counts[key] > 0 && (
          <span
            key={key}
            className="td-progress-seg"
            data-state={key}
            style={{ width: `${(counts[key] / total) * 100}%` }}
          />
        ))}
      </div>
      <div className="td-progress-legend">
        {SEGMENTS.map(key => (
          <span key={key} className="td-legend-item" data-state={key}>
            <span className="td-legend-dot" data-state={key} aria-hidden="true" />
            {t(`techDebt.legend.${key}`)} <strong>{counts[key]}</strong>
          </span>
        ))}
        <span className="td-legend-item" data-state="pending">
          <span className="td-legend-dot" data-state="pending" aria-hidden="true" />
          {t('techDebt.legend.pending')} <strong>{counts.pending}</strong>
        </span>
      </div>
    </div>
  );
}
