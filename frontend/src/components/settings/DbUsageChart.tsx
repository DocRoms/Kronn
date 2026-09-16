import { useCallback, useMemo, useState } from 'react';
import { BarChart3 } from 'lucide-react';
import { config } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import type { DbUsage } from '../../types/generated';

/** How many tables get their own bar. The tail is folded into one entry rather
 *  than dropped: a chart that silently omits rows would answer "what should I
 *  purge" with a number that does not add up to the file. */
const CHARTED = 8;

/** Fixed hues rather than a palette lookup: the tables are not a known set, so
 *  there is nothing to key a palette on. Spread across the wheel so adjacent
 *  segments stay distinguishable, at a saturation and lightness that read on
 *  both themes. */
function sliceColor(index: number): string {
  return `hsl(${(index * 47) % 360}, 58%, 52%)`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} o`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} Ko`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} Mo`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} Go`;
}

/** Bytes per row, which is what separates "many small rows" from "a few huge
 *  ones" — the distinction a row count cannot make and the one that decides
 *  whether purging a table is worth anything. */
function perRow(bytes: number, rows: number): string | null {
  return rows > 0 ? formatBytes(Math.round(bytes / rows)) : null;
}

export function DbUsageChart() {
  const { t } = useT();
  const [usage, setUsage] = useState<DbUsage | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Never on mount: dbstat walks the b-trees (~1 s on a 7 GB database), and a
  // settings page that costs a second of disk to open is its own problem.
  const measure = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setUsage(await config.dbUsage());
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  const slices = useMemo(() => {
    if (!usage) return [];
    const head = usage.tables.slice(0, CHARTED);
    const tail = usage.tables.slice(CHARTED);
    const tailBytes = tail.reduce((sum, table) => sum + table.bytes, 0);
    const tailRows = tail.reduce((sum, table) => sum + table.rows, 0);
    return tailBytes > 0
      ? [...head, { name: t('config.dbUsage.otherTables', tail.length), bytes: tailBytes, index_bytes: 0, rows: tailRows }]
      : head;
  }, [usage, t]);

  // The denominator is what the slices actually cover, so the percentages add
  // up to 100 instead of quietly losing the free pages to rounding.
  const chartedBytes = slices.reduce((sum, slice) => sum + slice.bytes, 0);

  if (!usage) {
    return (
      <div className="set-db-usage">
        <button className="set-action-btn" onClick={measure} disabled={loading}>
          <BarChart3 size={14} />
          {loading ? t('config.dbUsage.measuring') : t('config.dbUsage.measure')}
        </button>
        <small className="text-dim">{t('config.dbUsage.hint')}</small>
        {error && <p className="set-db-usage-error">{error}</p>}
      </div>
    );
  }

  return (
    <div className="set-db-usage">
      <div className="set-usage-subtitle">{t('config.dbUsage.title')}</div>

      <div className="set-usage-bar-container" role="img" aria-label={t('config.dbUsage.title')}>
        {slices.map((slice, index) => (
          <div
            key={slice.name}
            className="set-usage-bar-segment"
            style={{ width: `${Math.max(1, (slice.bytes / chartedBytes) * 100)}%`, background: sliceColor(index) }}
            title={`${slice.name} — ${formatBytes(slice.bytes)}`}
          />
        ))}
      </div>

      <table className="set-db-usage-table">
        <tbody>
          {slices.map((slice, index) => (
            <tr key={slice.name}>
              <td>
                <span className="set-usage-dot" style={{ background: sliceColor(index) }} />
                <span className="text-sm text-primary">{slice.name}</span>
              </td>
              <td className="text-sm text-primary set-db-usage-num">{formatBytes(slice.bytes)}</td>
              <td className="text-sm text-muted set-db-usage-num">
                {((slice.bytes / chartedBytes) * 100).toFixed(1)} %
              </td>
              <td className="text-sm text-muted set-db-usage-num">
                {t('config.dbUsage.rows', slice.rows.toLocaleString())}
              </td>
              <td className="text-sm text-dim set-db-usage-num">
                {perRow(slice.bytes, slice.rows) ?? '—'}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <div className="set-db-usage-footer">
        <span>{t('config.dbUsage.file')} {formatBytes(usage.file_bytes)}</span>
        {usage.wal_bytes > 0 && <span>{t('config.dbUsage.wal')} {formatBytes(usage.wal_bytes)}</span>}
        {usage.free_bytes > 0 && (
          <span title={t('config.dbUsage.freeHint')}>
            {t('config.dbUsage.free')} {formatBytes(usage.free_bytes)}
          </span>
        )}
        <button className="set-action-btn" onClick={measure} disabled={loading}>
          {loading ? t('config.dbUsage.measuring') : t('config.dbUsage.remeasure')}
        </button>
      </div>
    </div>
  );
}
