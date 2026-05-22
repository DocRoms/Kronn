// 0.8.6 (#61) — Read surface for the api_call_logs table populated since
// 0.8.6 phase 2 (broker calls) + 0.8.6 (workflow + manual_test calls).
//
// Lightweight: filter bar + table + click-to-drawer. Auto-refresh every
// 10s (toggle). Excerpts are pre-redacted server-side via core::redact.
//
// Design tradeoff: NOT a real-time stream. Polling keeps the
// implementation 1-file and is fine for a debug surface (you open it
// after the fact). If the volume justifies it, we can swap for SSE
// later.

import { useState, useEffect, useCallback, useRef } from 'react';
import { useT } from '../lib/I18nContext';
import { apiCallLogs, type ApiCallLogRow, type ApiCallLogsFilter } from '../lib/api';
import { RefreshCw, X, Filter, Activity } from 'lucide-react';
import './ApiCallLogsPage.css';

type SourceFilter = ApiCallLogsFilter['source'] | 'all';
type StatusFilter = ApiCallLogsFilter['status'] | 'all';

const REFRESH_INTERVAL_MS = 10_000;

export function ApiCallLogsPage() {
  const { t } = useT();
  const [rows, setRows] = useState<ApiCallLogRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [sourceFilter, setSourceFilter] = useState<SourceFilter>('all');
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all');
  const [pluginFilter, setPluginFilter] = useState<string>('');
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [drawerRow, setDrawerRow] = useState<ApiCallLogRow | null>(null);
  // Keep the fetcher stable in the interval cb : closures capturing
  // filter state would freeze the first values otherwise.
  const filterRef = useRef({ sourceFilter, statusFilter, pluginFilter });
  filterRef.current = { sourceFilter, statusFilter, pluginFilter };

  const refresh = useCallback(async () => {
    const { sourceFilter: src, statusFilter: st, pluginFilter: pl } = filterRef.current;
    const filter: ApiCallLogsFilter = {
      ...(src !== 'all' ? { source: src } : {}),
      ...(st !== 'all' ? { status: st } : {}),
      ...(pl.trim() !== '' ? { plugin_slug: pl.trim() } : {}),
      limit: 100,
    };
    setError(null);
    try {
      const data = await apiCallLogs.list(filter);
      setRows(data);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  // Initial fetch + on filter change.
  useEffect(() => {
    setLoading(true);
    refresh();
  }, [refresh, sourceFilter, statusFilter, pluginFilter]);

  // Auto-refresh loop.
  useEffect(() => {
    if (!autoRefresh) return;
    const id = setInterval(refresh, REFRESH_INTERVAL_MS);
    return () => clearInterval(id);
  }, [autoRefresh, refresh]);

  return (
    <div className="api-call-logs-page" data-testid="api-call-logs-page">
      <header className="api-call-logs-header">
        <h1 className="api-call-logs-title">
          <Activity size={18} style={{ marginRight: 8 }} />
          {t('apiCallLogs.title')}
        </h1>
        <p className="api-call-logs-subtitle">{t('apiCallLogs.subtitle')}</p>
      </header>

      {/* Filter bar */}
      <div className="api-call-logs-filters" data-testid="api-call-logs-filters">
        <div className="api-call-logs-filter-group">
          <label className="api-call-logs-filter-label">
            <Filter size={11} /> {t('apiCallLogs.filterSource')}
          </label>
          <div className="api-call-logs-chip-row">
            {(['all', 'workflow', 'agent_broker', 'manual_test'] as const).map(s => (
              <button
                key={s}
                type="button"
                className={`api-call-logs-chip${sourceFilter === s ? ' api-call-logs-chip-active' : ''}`}
                onClick={() => setSourceFilter(s)}
                data-testid={`api-call-logs-source-${s}`}
              >
                {t(`apiCallLogs.source.${s}`)}
              </button>
            ))}
          </div>
        </div>
        <div className="api-call-logs-filter-group">
          <label className="api-call-logs-filter-label">{t('apiCallLogs.filterStatus')}</label>
          <div className="api-call-logs-chip-row">
            {(['all', 'OK', 'ERROR', 'RateLimited', 'TimedOut'] as const).map(s => (
              <button
                key={s}
                type="button"
                className={`api-call-logs-chip${statusFilter === s ? ' api-call-logs-chip-active' : ''}`}
                onClick={() => setStatusFilter(s)}
                data-testid={`api-call-logs-status-${s}`}
              >
                {s === 'all' ? t('apiCallLogs.source.all') : s}
              </button>
            ))}
          </div>
        </div>
        <div className="api-call-logs-filter-group">
          <label className="api-call-logs-filter-label" htmlFor="api-call-logs-plugin-filter">
            {t('apiCallLogs.filterPlugin')}
          </label>
          <input
            id="api-call-logs-plugin-filter"
            className="api-call-logs-input"
            placeholder={t('apiCallLogs.filterPluginPlaceholder')}
            value={pluginFilter}
            onChange={e => setPluginFilter(e.target.value)}
            data-testid="api-call-logs-plugin-input"
          />
        </div>
        <div className="api-call-logs-filter-group api-call-logs-controls">
          <label className="api-call-logs-toggle">
            <input
              type="checkbox"
              checked={autoRefresh}
              onChange={e => setAutoRefresh(e.target.checked)}
              data-testid="api-call-logs-auto-refresh"
            />
            <span>{t('apiCallLogs.autoRefresh')}</span>
          </label>
          <button
            type="button"
            className="api-call-logs-refresh-btn"
            onClick={refresh}
            data-testid="api-call-logs-refresh"
          >
            <RefreshCw size={12} /> {t('apiCallLogs.refresh')}
          </button>
          {/* 0.8.6 — manual purge button. The backend already auto-purges
              rows > 90 days at boot ; this lets the user trim tighter
              (default 30 days) when they want a fresh start. */}
          <button
            type="button"
            className="api-call-logs-refresh-btn"
            onClick={async () => {
              const days = parseInt(prompt(t('apiCallLogs.purgePrompt'), '30') ?? '', 10);
              if (Number.isNaN(days) || days < 1) return;
              try {
                const removed = await apiCallLogs.purge(days);
                alert(t('apiCallLogs.purgeDone', String(removed), String(days)));
                refresh();
              } catch (e) {
                alert(t('apiCallLogs.purgeFailed', String(e)));
              }
            }}
            data-testid="api-call-logs-purge"
          >
            🗑 {t('apiCallLogs.purgeBtn')}
          </button>
        </div>
      </div>

      {/* Table */}
      <div className="api-call-logs-body">
        {loading && rows.length === 0 ? (
          <div className="api-call-logs-empty" data-testid="api-call-logs-loading">{t('apiCallLogs.loading')}</div>
        ) : error ? (
          <div className="api-call-logs-error" data-testid="api-call-logs-error">{error}</div>
        ) : rows.length === 0 ? (
          <div className="api-call-logs-empty" data-testid="api-call-logs-empty">{t('apiCallLogs.empty')}</div>
        ) : (
          <table className="api-call-logs-table" data-testid="api-call-logs-table">
            <thead>
              <tr>
                <th>{t('apiCallLogs.col.calledAt')}</th>
                <th>{t('apiCallLogs.col.source')}</th>
                <th>{t('apiCallLogs.col.plugin')}</th>
                <th>{t('apiCallLogs.col.endpoint')}</th>
                <th>{t('apiCallLogs.col.status')}</th>
                <th>{t('apiCallLogs.col.duration')}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map(row => (
                <tr
                  key={row.id}
                  className={`api-call-logs-row api-call-logs-row-${row.status.toLowerCase()}`}
                  onClick={() => setDrawerRow(row)}
                  data-testid={`api-call-logs-row-${row.id}`}
                >
                  <td className="api-call-logs-mono">{row.called_at}</td>
                  <td>
                    <span className={`api-call-logs-source-pill api-call-logs-source-${row.source}`}>
                      {row.source}
                    </span>
                  </td>
                  <td className="api-call-logs-mono">{row.plugin_slug}</td>
                  <td className="api-call-logs-mono">{row.method} {row.endpoint_path}</td>
                  <td>
                    <span className={`api-call-logs-status-pill api-call-logs-status-${row.status.toLowerCase()}`}>
                      {row.status}
                      {row.http_status !== null ? ` (${row.http_status})` : ''}
                    </span>
                  </td>
                  <td className="api-call-logs-mono">{row.duration_ms}ms</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* Detail drawer */}
      {drawerRow && (
        <div
          className="api-call-logs-drawer-backdrop"
          onClick={() => setDrawerRow(null)}
          data-testid="api-call-logs-drawer"
        >
          <div className="api-call-logs-drawer" onClick={e => e.stopPropagation()}>
            <div className="api-call-logs-drawer-header">
              <h2>{drawerRow.method} {drawerRow.endpoint_path}</h2>
              <button
                type="button"
                className="api-call-logs-drawer-close"
                onClick={() => setDrawerRow(null)}
                aria-label={t('common.close')}
                data-testid="api-call-logs-drawer-close"
              >
                <X size={14} />
              </button>
            </div>
            <dl className="api-call-logs-drawer-meta">
              <dt>{t('apiCallLogs.col.source')}</dt><dd>{drawerRow.source}</dd>
              <dt>{t('apiCallLogs.col.plugin')}</dt><dd>{drawerRow.plugin_slug}</dd>
              <dt>{t('apiCallLogs.col.status')}</dt>
              <dd>
                {drawerRow.status}
                {drawerRow.http_status !== null ? ` (HTTP ${drawerRow.http_status})` : ''}
              </dd>
              <dt>{t('apiCallLogs.col.duration')}</dt><dd>{drawerRow.duration_ms}ms</dd>
              {drawerRow.project_id && (<><dt>{t('apiCallLogs.col.project')}</dt><dd>{drawerRow.project_id}</dd></>)}
              {drawerRow.run_id && (<><dt>{t('apiCallLogs.col.run')}</dt><dd>{drawerRow.run_id}</dd></>)}
              {drawerRow.disc_id && (<><dt>{t('apiCallLogs.col.disc')}</dt><dd>{drawerRow.disc_id}</dd></>)}
              {drawerRow.agent && (<><dt>{t('apiCallLogs.col.agent')}</dt><dd>{drawerRow.agent}</dd></>)}
              <dt>{t('apiCallLogs.col.calledAt')}</dt><dd className="api-call-logs-mono">{drawerRow.called_at}</dd>
            </dl>
            <h3 className="api-call-logs-drawer-section">{t('apiCallLogs.drawer.request')}</h3>
            <pre className="api-call-logs-excerpt" data-testid="api-call-logs-request-excerpt">
              {drawerRow.request_excerpt ?? t('apiCallLogs.drawer.noRequest')}
            </pre>
            <h3 className="api-call-logs-drawer-section">{t('apiCallLogs.drawer.response')}</h3>
            <pre className="api-call-logs-excerpt" data-testid="api-call-logs-response-excerpt">
              {drawerRow.response_excerpt ?? t('apiCallLogs.drawer.noResponse')}
            </pre>
            {drawerRow.error_message && (
              <>
                <h3 className="api-call-logs-drawer-section">{t('apiCallLogs.drawer.error')}</h3>
                <pre className="api-call-logs-excerpt api-call-logs-error-block" data-testid="api-call-logs-error-block">
                  {drawerRow.error_message}
                </pre>
              </>
            )}
            <p className="api-call-logs-drawer-redaction-hint">{t('apiCallLogs.drawer.redactionHint')}</p>
          </div>
        </div>
      )}
    </div>
  );
}
