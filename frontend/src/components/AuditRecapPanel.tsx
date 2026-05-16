import { useEffect, useMemo, useState } from 'react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { ClipboardList, X, ChevronRight, ChevronDown } from 'lucide-react';
import './AuditRecapPanel.css';

type Step = {
  step_index: number;
  file_label: string;
  duration_ms?: number | null;
  step_tokens?: number | null;
  cumulative_tokens?: number | null;
  cli_success: boolean;
  step_warning?: string | null;
  step_repaired_from_template: boolean;
};

type AuditRun = {
  id: string;
  kind: string;
  agent_type: string;
  started_at: string;
  ended_at?: string | null;
  duration_ms?: number | null;
  status: string;
  td_total: number;
  health_score?: number | null;
};

type SortKey = 'step_index' | 'duration_ms' | 'step_tokens';

interface Props {
  projectId: string;
  /** Bumped by parent when an audit completes; triggers history refetch. */
  refreshTrigger?: number;
}

function fmtDuration(ms: number | null | undefined): string {
  if (ms == null) return '—';
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rs = Math.round(s - m * 60);
  return `${m}m${String(rs).padStart(2, '0')}`;
}

function fmtTokens(n: number | null | undefined): string {
  if (n == null) return '—';
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

function fmtRelativeDate(iso: string, locale: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleString(locale, {
      day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit',
    });
  } catch {
    return iso;
  }
}

function kindIcon(kind: string): string {
  switch (kind) {
    case 'Full':          return '🌐';
    case 'Security':      return '🛡';
    case 'Docker':        return '🐳';
    case 'Performance':   return '⚡';
    case 'Accessibility': return '👁';
    case 'Rgaa':          return '♿';
    case 'Database':      return '🗄';
    case 'ApiDesign':     return '🔌';
    default:              return '📋';
  }
}

/**
 * 0.8.4 (#332 + #333) — themed lateral drawer for audit history.
 *
 * Pre-fix #333: the drawer used hardcoded inline `rgba()` + hex
 * values, which broke every non-dark theme (light, sakura, matrix,
 * batman). All styling now lives in `AuditRecapPanel.css` and uses
 * `--kr-*` tokens so the panel adapts to whichever theme the user
 * picked.
 */
export default function AuditRecapPanel({ projectId, refreshTrigger }: Props) {
  const { t, locale } = useT();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [history, setHistory] = useState<AuditRun[]>([]);
  const [expandedRunId, setExpandedRunId] = useState<string | null>(null);
  const [steps, setSteps] = useState<Step[]>([]);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const [loadingSteps, setLoadingSteps] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sortKey, setSortKey] = useState<SortKey>('step_index');
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>('asc');
  const [kindFilter, setKindFilter] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  // Fetch history on mount + every refreshTrigger bump.
  useEffect(() => {
    let cancelled = false;
    setLoadingHistory(true);
    setError(null);
    (async () => {
      try {
        const rows = await projectsApi.auditHistory(projectId);
        if (cancelled) return;
        setHistory(rows ?? []);
      } catch (e) {
        if (!cancelled) setError(String((e as Error).message ?? e));
      } finally {
        if (!cancelled) setLoadingHistory(false);
      }
    })();
    return () => { cancelled = true; };
  }, [projectId, refreshTrigger]);

  // Load steps for the expanded card.
  useEffect(() => {
    if (!expandedRunId) { setSteps([]); return; }
    let cancelled = false;
    setLoadingSteps(true);
    (async () => {
      try {
        const rows = await projectsApi.auditRunSteps(expandedRunId);
        if (cancelled) return;
        setSteps(rows ?? []);
      } catch (e) {
        if (!cancelled) setError(String((e as Error).message ?? e));
      } finally {
        if (!cancelled) setLoadingSteps(false);
      }
    })();
    return () => { cancelled = true; };
  }, [expandedRunId]);

  // Close drawer on Escape.
  useEffect(() => {
    if (!drawerOpen) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setDrawerOpen(false); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [drawerOpen]);

  const kindsInStrip = useMemo(() => {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const r of history) {
      if (!seen.has(r.kind)) { seen.add(r.kind); out.push(r.kind); }
    }
    return out;
  }, [history]);

  const filteredHistory = useMemo(() => (
    kindFilter ? history.filter(r => r.kind === kindFilter) : history
  ), [history, kindFilter]);

  const visibleRuns = useMemo(() => (
    showAll ? filteredHistory : filteredHistory.slice(0, 10)
  ), [filteredHistory, showAll]);
  const hiddenCount = filteredHistory.length - visibleRuns.length;

  const sortedSteps = useMemo(() => {
    const copy = [...steps];
    copy.sort((a, b) => {
      const av = (a[sortKey] ?? 0) as number;
      const bv = (b[sortKey] ?? 0) as number;
      const cmp = av - bv;
      return sortDir === 'asc' ? cmp : -cmp;
    });
    return copy;
  }, [steps, sortKey, sortDir]);

  const toggleSort = (key: SortKey) => {
    if (sortKey === key) setSortDir(sortDir === 'asc' ? 'desc' : 'asc');
    else { setSortKey(key); setSortDir(key === 'step_index' ? 'asc' : 'desc'); }
  };
  const sortMark = (key: SortKey) => sortKey === key ? (sortDir === 'asc' ? ' ▲' : ' ▼') : '';

  if (history.length === 0 && !loadingHistory) return null;

  return (
    <>
      <button
        type="button"
        className="arp-cta"
        data-testid="audit-recap-toggle"
        onClick={() => setDrawerOpen(true)}
      >
        <ClipboardList size={12} />
        {t('projects.docAi.auditRecap.openDrawer', history.length)}
      </button>

      {drawerOpen && (
        <>
          <div
            className="arp-overlay"
            data-testid="audit-recap-overlay"
            onClick={() => setDrawerOpen(false)}
          />
          <aside
            className="arp-drawer"
            data-testid="audit-recap-drawer"
            role="dialog"
            aria-label={t('projects.docAi.auditRecap.drawerTitle')}
          >
            <header className="arp-drawer-header">
              <h2 className="arp-drawer-title">
                {t('projects.docAi.auditRecap.drawerTitle')}
                <span className="arp-drawer-title-count">({history.length})</span>
              </h2>
              <button
                type="button"
                className="arp-drawer-close"
                onClick={() => setDrawerOpen(false)}
                aria-label={t('common.close')}
                data-testid="audit-recap-drawer-close"
              >
                <X size={16} />
              </button>
            </header>

            <div className="arp-drawer-body">
              {loadingHistory && <p>{t('common.loading')}</p>}
              {error && <p className="arp-error">{error}</p>}

              {history.length > 0 && kindsInStrip.length > 1 && (
                <div
                  className="arp-filter-row"
                  data-testid="audit-recap-kind-filter"
                >
                  <button
                    type="button"
                    className="arp-filter-pill"
                    data-active={kindFilter === null ? 'true' : 'false'}
                    data-testid="audit-recap-filter-all"
                    onClick={() => setKindFilter(null)}
                  >
                    {t('projects.docAi.auditRecap.filterAll')}
                  </button>
                  {kindsInStrip.map(kind => {
                    const count = history.filter(r => r.kind === kind).length;
                    const isActive = kindFilter === kind;
                    return (
                      <button
                        key={kind}
                        type="button"
                        className="arp-filter-pill"
                        data-active={isActive ? 'true' : 'false'}
                        data-testid={`audit-recap-filter-${kind}`}
                        onClick={() => setKindFilter(isActive ? null : kind)}
                      >
                        {kindIcon(kind)} {kind} ({count})
                      </button>
                    );
                  })}
                </div>
              )}

              <div className="arp-runs">
                {visibleRuns.map(run => {
                  const isExpanded = expandedRunId === run.id;
                  const failed = run.status !== 'Completed';
                  return (
                    <div
                      key={run.id}
                      className="arp-run-card"
                      data-active={isExpanded ? 'true' : 'false'}
                      data-failed={failed ? 'true' : 'false'}
                    >
                      <button
                        type="button"
                        className="arp-run-card-btn"
                        data-testid={`audit-recap-chip-${run.id}`}
                        onClick={() => setExpandedRunId(isExpanded ? null : run.id)}
                      >
                        <div className="arp-run-card-info">
                          <div className="arp-run-card-title">
                            <span aria-hidden>{kindIcon(run.kind)}</span>
                            <span>{run.kind}</span>
                            <span className="arp-run-card-time">
                              · {fmtRelativeDate(run.started_at, locale)}
                            </span>
                          </div>
                          <div className="arp-run-meta">
                            <span title={t('projects.docAi.auditRecap.metaDurationTooltip')}>
                              ⏱ {fmtDuration(run.duration_ms)}
                            </span>
                            {run.td_total > 0 && (
                              <span title={t('projects.docAi.auditRecap.metaTdTooltip')}>
                                🐛 {run.td_total} TD
                              </span>
                            )}
                            {run.health_score != null && (
                              <span title={t('projects.docAi.auditRecap.metaHealthTooltip')}>
                                ❤ {run.health_score}/100
                              </span>
                            )}
                            {failed && <span className="arp-run-status-failed">· {run.status}</span>}
                          </div>
                        </div>
                        {isExpanded
                          ? <ChevronDown size={14} className="arp-run-chevron" />
                          : <ChevronRight size={14} className="arp-run-chevron" />}
                      </button>

                      {isExpanded && (
                        <div className="arp-run-steps">
                          {loadingSteps && <p>{t('common.loading')}</p>}
                          {!loadingSteps && steps.length === 0 && (
                            <p data-testid="audit-recap-empty">
                              {t('projects.docAi.auditRecap.empty')}
                            </p>
                          )}
                          {!loadingSteps && steps.length > 0 && (
                            <table
                              className="arp-steps-table"
                              data-testid="audit-recap-table"
                            >
                              <thead>
                                <tr>
                                  <th
                                    className="arp-num"
                                    data-sortable="true"
                                    onClick={() => toggleSort('step_index')}
                                    title={t('projects.docAi.auditRecap.colStepTooltip')}
                                  >
                                    #{sortMark('step_index')}
                                  </th>
                                  <th title={t('projects.docAi.auditRecap.colFileTooltip')}>
                                    {t('projects.docAi.auditRecap.colFile')}
                                  </th>
                                  <th
                                    className="arp-num"
                                    data-sortable="true"
                                    onClick={() => toggleSort('duration_ms')}
                                    title={t('projects.docAi.auditRecap.colDurationTooltip')}
                                  >
                                    {t('projects.docAi.auditRecap.colDuration')}{sortMark('duration_ms')}
                                  </th>
                                  <th
                                    className="arp-num"
                                    data-sortable="true"
                                    onClick={() => toggleSort('step_tokens')}
                                    title={t('projects.docAi.auditRecap.colTokensTooltip')}
                                  >
                                    {t('projects.docAi.auditRecap.colTokens')}{sortMark('step_tokens')}
                                  </th>
                                </tr>
                              </thead>
                              <tbody>
                                {sortedSteps.map(s => {
                                  const sf = !s.cli_success || !!s.step_warning;
                                  return (
                                    <tr
                                      key={s.step_index}
                                      data-failed={sf ? 'true' : 'false'}
                                      data-testid={`audit-recap-row-${s.step_index}`}
                                      title={s.step_warning ?? undefined}
                                    >
                                      <td className="arp-num">{s.step_index}</td>
                                      <td>
                                        {s.file_label}
                                        {s.step_repaired_from_template && (
                                          <span
                                            className="arp-repair-icon"
                                            title={t('projects.docAi.auditRecap.repairedHint')}
                                          >🔧</span>
                                        )}
                                      </td>
                                      <td className="arp-num">{fmtDuration(s.duration_ms)}</td>
                                      <td className="arp-num">{fmtTokens(s.step_tokens)}</td>
                                    </tr>
                                  );
                                })}
                              </tbody>
                            </table>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>

              {hiddenCount > 0 && !showAll && (
                <button
                  type="button"
                  className="arp-show-more"
                  data-testid="audit-recap-chips-show-more"
                  onClick={() => setShowAll(true)}
                >
                  {t('projects.docAi.auditRecap.showMoreChips', hiddenCount)}
                </button>
              )}
              {showAll && filteredHistory.length > 10 && (
                <button
                  type="button"
                  className="arp-show-more"
                  data-testid="audit-recap-chips-show-less"
                  onClick={() => setShowAll(false)}
                >
                  {t('projects.docAi.auditRecap.showLessChips')}
                </button>
              )}
            </div>
          </aside>
        </>
      )}
    </>
  );
}
