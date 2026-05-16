/**
 * 0.8.5 — Quick Prompt version history + metrics drawer.
 *
 * Lateral drawer (CTA opens, Escape closes, click-outside closes)
 * showing the version timeline of a QP + per-version aggregated
 * launch metrics (avg tokens, avg duration, avg cost, launch count,
 * Δ% vs the immediately previous version).
 *
 * Pattern mirrored from `AuditRecapPanel` (0.8.4 #332/#333) for
 * visual + behavioural consistency: same `--kr-*` token discipline,
 * same drawer chrome (`.qph-*` class prefix), same expand-card UX.
 *
 * Pertinence-Δ discipline: deltas are ONLY emitted when both versions
 * have ≥ 3 launches. Below that floor the noise drowns the signal —
 * a single fast run shouldn't make v3 look 60% "better" than v2.
 *
 * Diff viewer: per-line LCS-free heuristic (split on newline, mark
 * removed-only / added-only / unchanged). Good enough for the
 * "what did the AI improver actually change?" use case without
 * pulling in a library. Side-by-side rendering with vertical
 * alignment by line index — not perfect for large reorderings but
 * correct for the typical "tighten wording" refactor.
 */
import { useEffect, useMemo, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { quickPrompts as quickPromptsApi } from '../lib/api';
import type { QuickPromptVersion, QuickPromptVersionMetrics } from '../types/generated';
import { History, X, ChevronRight, ChevronDown, Trash2 } from 'lucide-react';
import './QPHistoryDrawer.css';

interface Props {
  qpId: string;
  qpName: string;
  /** Bumped by the parent when the QP is updated; triggers a refetch.
   *  Mirrors `AuditRecapPanel.refreshTrigger`. */
  refreshTrigger?: number;
}

/** Pertinence-Δ noise floor. Below this many launches per side, the
 *  Δ% chip is hidden (renders "—" instead). */
const DELTA_MIN_LAUNCHES = 3;

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

function fmtCost(usd: number | null | undefined): string {
  if (usd == null) return '—';
  if (usd < 0.01) return `<$0.01`;
  if (usd < 1) return `$${usd.toFixed(3)}`;
  return `$${usd.toFixed(2)}`;
}

function fmtDate(iso: string, locale: string): string {
  try {
    return new Date(iso).toLocaleString(locale, {
      day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit',
    });
  } catch {
    return iso;
  }
}

/** Compute % delta between current and previous. Returns `null` when
 *  either side has < DELTA_MIN_LAUNCHES OR previous is zero (division). */
function computeDelta(
  cur: number | null | undefined,
  prev: number | null | undefined,
  curLaunches: number,
  prevLaunches: number,
): number | null {
  if (cur == null || prev == null) return null;
  if (curLaunches < DELTA_MIN_LAUNCHES || prevLaunches < DELTA_MIN_LAUNCHES) return null;
  if (prev === 0) return null;
  return Math.round(((cur - prev) / prev) * 100);
}

/** Side-by-side line diff. Splits both strings on `\n` and pads the
 *  shorter list. Each row carries a `kind`:
 *    - 'same'    : both sides have identical text
 *    - 'changed' : both sides have text but they differ
 *    - 'added'   : only `next` has text (left side empty)
 *    - 'removed' : only `prev` has text (right side empty)
 *  Pure helper, exported for unit testing. */
export interface DiffRow {
  prev: string;
  next: string;
  kind: 'same' | 'changed' | 'added' | 'removed';
}
export function diffLines(prev: string, next: string): DiffRow[] {
  const pLines = prev.split('\n');
  const nLines = next.split('\n');
  const max = Math.max(pLines.length, nLines.length);
  const out: DiffRow[] = [];
  for (let i = 0; i < max; i++) {
    const p = pLines[i] ?? '';
    const n = nLines[i] ?? '';
    let kind: DiffRow['kind'];
    if (p === n) kind = 'same';
    else if (p === '') kind = 'added';
    else if (n === '') kind = 'removed';
    else kind = 'changed';
    out.push({ prev: p, next: n, kind });
  }
  return out;
}

export default function QPHistoryDrawer({ qpId, qpName, refreshTrigger }: Props) {
  const { t, locale } = useT();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [history, setHistory] = useState<QuickPromptVersion[]>([]);
  const [metrics, setMetrics] = useState<QuickPromptVersionMetrics[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expandedVersion, setExpandedVersion] = useState<number | null>(null);
  const [diffOpen, setDiffOpen] = useState<number | null>(null);
  const [deletingVersion, setDeletingVersion] = useState<number | null>(null);

  /**
   * 0.8.5 — delete an archived version after user confirmation.
   * The backend refuses the current version (returns success:false with
   * an explanatory error); we surface that as-is. After success, refetch
   * both lists so the card disappears + Δ% chips on other versions
   * recompute against the new "previous" sibling.
   */
  const handleDeleteVersion = async (version_index: number) => {
    if (deletingVersion != null) return;
    const ok = window.confirm(t('qp.history.deleteConfirm', version_index));
    if (!ok) return;
    setDeletingVersion(version_index);
    try {
      await quickPromptsApi.deleteVersion(qpId, version_index);
      const [hist, met] = await Promise.all([
        quickPromptsApi.history(qpId),
        quickPromptsApi.metrics(qpId),
      ]);
      setHistory(hist ?? []);
      setMetrics(met ?? []);
      setVersionCount((hist ?? []).length);
      if (expandedVersion === version_index) setExpandedVersion(null);
      if (diffOpen === version_index) setDiffOpen(null);
    } catch (e) {
      setError(String((e as Error).message ?? e));
    } finally {
      setDeletingVersion(null);
    }
  };

  // Fetch history + metrics on open AND whenever the parent bumps the trigger.
  useEffect(() => {
    if (!drawerOpen) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    (async () => {
      try {
        const [hist, met] = await Promise.all([
          quickPromptsApi.history(qpId),
          quickPromptsApi.metrics(qpId),
        ]);
        if (cancelled) return;
        setHistory(hist ?? []);
        setMetrics(met ?? []);
      } catch (e) {
        if (!cancelled) setError(String((e as Error).message ?? e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [drawerOpen, qpId, refreshTrigger]);

  // Close on Escape.
  useEffect(() => {
    if (!drawerOpen) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setDrawerOpen(false); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [drawerOpen]);

  // Index metrics by version_index for O(1) lookup when rendering.
  const metricsByVersion = useMemo(() => {
    const m = new Map<number, QuickPromptVersionMetrics>();
    for (const row of metrics) m.set(row.version_index, row);
    return m;
  }, [metrics]);

  // Pre-fetch the version count for the CTA badge. We DON'T gate the
  // CTA visibility on the count: post-059 backfill every QP has v1,
  // and even a freshly-created QP with no metrics yet is worth one
  // click to confirm "yes the timeline is there, just empty so far".
  const [versionCount, setVersionCount] = useState<number | null>(null);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const hist = await quickPromptsApi.history(qpId);
        if (!cancelled) setVersionCount((hist ?? []).length);
      } catch {
        if (!cancelled) setVersionCount(0);
      }
    })();
    return () => { cancelled = true; };
  }, [qpId, refreshTrigger]);

  return (
    <>
      <button
        type="button"
        className="qph-cta"
        data-testid="qp-history-toggle"
        onClick={() => setDrawerOpen(true)}
        title={t('qp.history.openTitle', qpName)}
      >
        <History size={12} />
        {t('qp.history.openLabel', versionCount ?? '…')}
      </button>

      {drawerOpen && (
        <>
          <div
            className="qph-overlay"
            data-testid="qp-history-overlay"
            onClick={() => setDrawerOpen(false)}
          />
          <aside
            className="qph-drawer"
            data-testid="qp-history-drawer"
            role="dialog"
            aria-label={t('qp.history.drawerTitle', qpName)}
          >
            <header className="qph-drawer-header">
              <h2 className="qph-drawer-title">
                {t('qp.history.drawerTitle', qpName)}
                <span className="qph-drawer-title-count">({history.length})</span>
              </h2>
              <button
                type="button"
                className="qph-drawer-close"
                onClick={() => setDrawerOpen(false)}
                aria-label={t('common.close')}
                data-testid="qp-history-drawer-close"
              >
                <X size={16} />
              </button>
            </header>

            <div className="qph-drawer-body">
              {loading && <p>{t('common.loading')}</p>}
              {error && <p className="qph-error">{error}</p>}

              {!loading && history.length === 0 && (
                <p className="qph-empty">{t('qp.history.empty')}</p>
              )}

              {history.length > 0 && (
                <div className="qph-versions">
                  {history.map((v, idx) => {
                    const expanded = expandedVersion === v.version_index;
                    const isCurrent = idx === 0;
                    const m = metricsByVersion.get(v.version_index);
                    // Previous-version metrics for the Δ chip. Newest-
                    // first ordering: idx+1 is the older sibling.
                    const prev = history[idx + 1];
                    const prevMetrics = prev ? metricsByVersion.get(prev.version_index) : undefined;
                    const dTokens = m && prevMetrics
                      ? computeDelta(m.avg_tokens, prevMetrics.avg_tokens, m.launches, prevMetrics.launches)
                      : null;
                    const dDur = m && prevMetrics
                      ? computeDelta(m.avg_duration_ms, prevMetrics.avg_duration_ms, m.launches, prevMetrics.launches)
                      : null;
                    return (
                      <div
                        key={v.id}
                        className="qph-version-card"
                        data-active={expanded ? 'true' : 'false'}
                        data-current={isCurrent ? 'true' : 'false'}
                        data-testid={`qp-history-version-${v.version_index}`}
                      >
                        <div className="qph-version-card-header-row">
                          <button
                            type="button"
                            className="qph-version-card-header"
                            onClick={() => setExpandedVersion(expanded ? null : v.version_index)}
                            aria-expanded={expanded}
                          >
                            {expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                            <span className="qph-version-tag">v{v.version_index}</span>
                            {isCurrent && (
                              <span className="qph-version-current-pill">{t('qp.history.current')}</span>
                            )}
                            <span className="qph-version-date">{fmtDate(v.created_at, locale)}</span>
                          </button>
                          {/* 0.8.5 — delete affordance, only on archived versions.
                              The current version is the anchor for the live QP
                              body so the backend refuses its deletion; we hide
                              the button entirely to avoid the failure path. */}
                          {!isCurrent && (
                            <button
                              type="button"
                              className="qph-version-delete"
                              data-testid={`qp-history-delete-${v.version_index}`}
                              onClick={(e) => { e.stopPropagation(); handleDeleteVersion(v.version_index); }}
                              disabled={deletingVersion != null}
                              aria-label={t('qp.history.deleteAria', v.version_index)}
                              title={t('qp.history.deleteAria', v.version_index)}
                            >
                              <Trash2 size={11} />
                            </button>
                          )}
                        </div>

                        <div className="qph-version-meta">
                          <span className="qph-meta-chip" title={t('qp.history.launchesTooltip')}>
                            🚀 {m ? m.launches : 0}
                          </span>
                          <span className="qph-meta-chip" title={t('qp.history.avgTokensTooltip')}>
                            💬 {m ? fmtTokens(m.avg_tokens) : '—'}
                            {dTokens != null && (
                              <span className={`qph-delta ${dTokens < 0 ? 'qph-delta-good' : dTokens > 0 ? 'qph-delta-bad' : ''}`}>
                                ({dTokens > 0 ? '+' : ''}{dTokens}%)
                              </span>
                            )}
                          </span>
                          <span className="qph-meta-chip" title={t('qp.history.avgDurationTooltip')}>
                            ⏱ {m ? fmtDuration(m.avg_duration_ms) : '—'}
                            {dDur != null && (
                              <span className={`qph-delta ${dDur < 0 ? 'qph-delta-good' : dDur > 0 ? 'qph-delta-bad' : ''}`}>
                                ({dDur > 0 ? '+' : ''}{dDur}%)
                              </span>
                            )}
                          </span>
                          <span className="qph-meta-chip" title={t('qp.history.avgCostTooltip')}>
                            💰 {m ? fmtCost(m.avg_cost_usd) : '—'}
                          </span>
                        </div>

                        {expanded && (
                          <div className="qph-version-body">
                            {v.description && (
                              <p className="qph-version-description">{v.description}</p>
                            )}
                            <div className="qph-bindings-row">
                              {v.skill_ids && v.skill_ids.length > 0 && (
                                <span className="qph-binding-chip">⚡ {v.skill_ids.join(', ')}</span>
                              )}
                              {v.profile_ids && v.profile_ids.length > 0 && (
                                <span className="qph-binding-chip">👤 {v.profile_ids.join(', ')}</span>
                              )}
                              {v.directive_ids && v.directive_ids.length > 0 && (
                                <span className="qph-binding-chip">📝 {v.directive_ids.join(', ')}</span>
                              )}
                            </div>
                            <div className="qph-version-template-actions">
                              {prev && (
                                <button
                                  type="button"
                                  className="qph-diff-toggle"
                                  data-testid={`qp-history-diff-toggle-${v.version_index}`}
                                  onClick={() => setDiffOpen(diffOpen === v.version_index ? null : v.version_index)}
                                >
                                  {diffOpen === v.version_index
                                    ? t('qp.history.hideDiff')
                                    : t('qp.history.showDiff', v.version_index - 1, v.version_index)}
                                </button>
                              )}
                            </div>
                            {diffOpen === v.version_index && prev ? (
                              <div className="qph-diff" data-testid={`qp-history-diff-${v.version_index}`}>
                                <div className="qph-diff-col qph-diff-col-prev">
                                  <header>{t('qp.history.diffPrev', prev.version_index)}</header>
                                  {diffLines(prev.prompt_template, v.prompt_template).map((row, i) => (
                                    <div key={i} className={`qph-diff-line qph-diff-line-${row.kind}`}>
                                      {row.prev || ' '}
                                    </div>
                                  ))}
                                </div>
                                <div className="qph-diff-col qph-diff-col-next">
                                  <header>{t('qp.history.diffNext', v.version_index)}</header>
                                  {diffLines(prev.prompt_template, v.prompt_template).map((row, i) => (
                                    <div key={i} className={`qph-diff-line qph-diff-line-${row.kind}`}>
                                      {row.next || ' '}
                                    </div>
                                  ))}
                                </div>
                              </div>
                            ) : (
                              <pre className="qph-template-preview">{v.prompt_template}</pre>
                            )}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </aside>
        </>
      )}
    </>
  );
}
