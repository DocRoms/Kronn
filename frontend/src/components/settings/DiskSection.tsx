// KT-1318 — what Kronn keeps on the disk, and how to take it back.
//
// A 1.8 TB disk reached 683 MB free because every parallel task lane builds in
// its own `target/wt-<lane>` (5-30 GB) and nothing ever removed them. This
// panel answers "where did the disk go" (free space, Kronn's usage by kind and
// by task) and offers the purge: orphans by default, a named entry only after a
// confirmation, never an active lane.
import { useCallback, useState } from 'react';
import { AlertTriangle, Gauge, RefreshCw, Trash2 } from 'lucide-react';
import { config as configApi, diskApi } from '../../lib/api';
import type { DiskEntry, DiskReport, DiskState } from '../../lib/api';
import { formatBytes } from '../../lib/weightFormat';
import type { ToastFn } from '../../hooks/useToast';
import '../../pages/SettingsPage.css';

interface DiskSectionProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

function idleLabel(seconds: number | null, t: DiskSectionProps['t']): string {
  if (seconds === null) return '—';
  if (seconds < 3600) return t('settings.disk.minutes', Math.floor(seconds / 60));
  if (seconds < 86_400) return t('settings.disk.hours', Math.floor(seconds / 3600));
  return t('settings.disk.days', Math.floor(seconds / 86_400));
}

function sizeLabel(bytes: number, partial: boolean): string {
  return `${partial ? '≥ ' : ''}${formatBytes(bytes)}`;
}

export function DiskSection({ toast, t }: DiskSectionProps) {
  const [report, setReport] = useState<DiskReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [launchMin, setLaunchMin] = useState('');
  const [idleHours, setIdleHours] = useState('');

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await diskApi.report();
      setReport(next);
      setLaunchMin(String(next.policy.launch_min_free_gib));
      setIdleHours(String(next.policy.sweep_idle_hours));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  // Never on mount: sizing the lane build dirs walks a lot of files, and a
  // settings page that costs that to open is its own problem. The report is
  // loaded by the "Measure" button.

  const purgeOrphans = async () => {
    if (busy || !report || report.orphan_count === 0) return;
    if (!window.confirm(t('settings.disk.confirmOrphans', report.orphan_count, formatBytes(report.orphan_bytes)))) return;
    setBusy(true);
    try {
      const result = await diskApi.purgeOrphans();
      toast(t('settings.disk.purged', result.items.filter(i => i.purged).length, formatBytes(result.freed_bytes)), 'success');
      await load();
    } catch (cause) {
      toast(cause instanceof Error ? cause.message : String(cause), 'error');
    } finally {
      setBusy(false);
    }
  };

  const purgeEntry = async (entry: DiskEntry) => {
    if (busy || !entry.purgeable) return;
    if (!window.confirm(t('settings.disk.confirmEntry', entry.path, formatBytes(entry.bytes)))) return;
    setBusy(true);
    try {
      const result = await diskApi.purgeEntries([entry.path], entry.project_id);
      const item = result.items[0];
      if (item?.purged) {
        toast(t('settings.disk.purged', 1, formatBytes(item.bytes)), 'success');
      } else {
        toast(t('settings.disk.refused', item?.reason ?? ''), 'error');
      }
      await load();
    } catch (cause) {
      toast(cause instanceof Error ? cause.message : String(cause), 'error');
    } finally {
      setBusy(false);
    }
  };

  const savePolicy = async () => {
    const min = Number(launchMin);
    const hours = Number(idleHours);
    if (!Number.isInteger(min) || min < 0 || !Number.isInteger(hours) || hours < 0) {
      toast(t('settings.disk.invalidPolicy'), 'error');
      return;
    }
    try {
      await configApi.setServerConfig({ disk_launch_min_free_gib: min, disk_sweep_idle_hours: hours });
      toast(t('settings.disk.policySaved'), 'success');
      await load();
    } catch (cause) {
      toast(cause instanceof Error ? cause.message : String(cause), 'error');
    }
  };

  const stateLabel = (state: DiskState) => t(`settings.disk.state.${state}`);

  return (
    <div id="settings-disk" className="set-card">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <Gauge size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.disk.title')}</span>
          {report && report.available_bytes !== null && (
            <span className="text-sm text-dim" style={{ marginLeft: 'auto' }} data-testid="disk-free">
              {t('settings.disk.free', formatBytes(report.available_bytes))}
            </span>
          )}
        </div>
        <p className="set-hint">{t('settings.disk.hint')}</p>

        {!report && (
          <button className="set-action-btn" onClick={load} disabled={loading} data-testid="disk-measure">
            <RefreshCw size={14} />
            {loading ? t('settings.disk.measuring') : t('settings.disk.measure')}
          </button>
        )}
        {error && <p className="set-db-usage-error" role="alert">{error}</p>}

        {report && (
          <>
            {report.under_pressure && (
              <div className="set-expose-warn" data-testid="disk-pressure">
                <AlertTriangle size={13} />
                <span>{t('settings.disk.pressure', report.policy.launch_min_free_gib)}</span>
              </div>
            )}

            <div className="flex-wrap gap-8 mb-8" data-testid="disk-totals">
              {report.totals.map(total => (
                <div key={total.kind}>
                  <div className="text-sm text-dim">{t(`settings.disk.kind.${total.kind}`)}</div>
                  <div className="font-semibold">{sizeLabel(total.bytes, total.bytes_are_partial)}</div>
                  <small className="text-dim">{t('settings.disk.entries', total.entries)}</small>
                </div>
              ))}
            </div>

            <div className="flex-row gap-4 mb-8">
              <button
                className="set-action-btn"
                onClick={purgeOrphans}
                disabled={busy || report.orphan_count === 0}
                data-testid="disk-purge-orphans"
              >
                <Trash2 size={14} />
                {t('settings.disk.purgeOrphans', report.orphan_count, formatBytes(report.orphan_bytes))}
              </button>
              <button className="set-action-btn" onClick={load} disabled={loading || busy}>
                <RefreshCw size={14} />
                {t('settings.disk.refresh')}
              </button>
              <small className="text-dim">{t('settings.disk.orphanRule', report.policy.sweep_idle_hours)}</small>
            </div>

            <table className="set-db-usage-table" data-testid="disk-entries">
              <thead>
                <tr>
                  <th align="left">{t('settings.disk.col.entry')}</th>
                  <th align="left">{t('settings.disk.col.owner')}</th>
                  <th className="set-db-usage-num">{t('settings.disk.col.size')}</th>
                  <th className="set-db-usage-num">{t('settings.disk.col.idle')}</th>
                  <th align="left">{t('settings.disk.col.state')}</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {report.entries.map(entry => (
                  <tr key={entry.path} data-testid="disk-entry" data-state={entry.state}>
                    <td title={entry.path}>
                      <span className="text-sm text-dim">{t(`settings.disk.kind.${entry.kind}`)}</span>{' '}
                      {entry.name}
                    </td>
                    <td>{entry.owner ? `${entry.owner.task_ref ?? entry.owner.execution_id.slice(0, 8)} · ${entry.owner.status}` : '—'}</td>
                    <td className="set-db-usage-num">{sizeLabel(entry.bytes, entry.bytes_are_partial)}</td>
                    <td className="set-db-usage-num">{idleLabel(entry.idle_seconds, t)}</td>
                    <td title={entry.reason ?? undefined}>{stateLabel(entry.state)}</td>
                    <td>
                      {entry.purgeable && (
                        <button
                          className="set-action-btn"
                          onClick={() => purgeEntry(entry)}
                          disabled={busy}
                          aria-label={t('settings.disk.deleteEntry', entry.name)}
                          data-testid="disk-delete-entry"
                        >
                          <Trash2 size={12} />
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>

            <div className="flex-row gap-4 mt-8" data-testid="disk-policy">
              <label className="text-sm">
                {t('settings.disk.launchMin')}{' '}
                <input
                  type="number"
                  min={0}
                  value={launchMin}
                  onChange={event => setLaunchMin(event.target.value)}
                  aria-label={t('settings.disk.launchMin')}
                />
              </label>
              <label className="text-sm">
                {t('settings.disk.idleHours')}{' '}
                <input
                  type="number"
                  min={0}
                  value={idleHours}
                  onChange={event => setIdleHours(event.target.value)}
                  aria-label={t('settings.disk.idleHours')}
                />
              </label>
              <button className="set-action-btn" onClick={savePolicy}>
                {t('settings.disk.savePolicy')}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
