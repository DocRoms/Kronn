// 0.8.7 — Agent usage & cost, presented as an RTK-style "eco mode" card.
//
// Data comes from `ccusage` (via GET /api/usage) — the REAL token + cache
// breakdown of the detected CLIs (Claude / Codex / Gemini …), read from
// their local logs. Replaces the old `estimate_cost`-based section, which
// only counted tokens passing THROUGH Kronn and ignored prompt caching
// (over-estimating ~6×). Global view (all sessions, CLI + Kronn); per-disc
// attribution is a deliberate follow-up.

import { useState, useEffect, useCallback } from 'react';
import { BarChart3, ExternalLink, ChevronDown, ChevronUp, ChevronLeft, ChevronRight, RefreshCw } from 'lucide-react';
import { usage as usageApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import type { UsageReport } from '../../types/generated';
import '../../pages/SettingsPage.css';

const CCUSAGE_GITHUB_URL = 'https://github.com/ryoppippi/ccusage';

type Period = 'daily' | 'weekly' | 'monthly';

/* ── Formatters ── */
function fmtCost(usd: number): string {
  if (usd === 0) return '$0.00';
  if (usd < 0.01) return '< $0.01';
  return `$${usd.toFixed(2)}`;
}
function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}k`;
  return String(n);
}

/* ── Period label ──
 * ccusage stamps only the *start* of each bucket: a `YYYY-MM-DD` Monday for
 * weekly, `YYYY-MM` for monthly. Daily is already a full date. We expand
 * weekly into a `start → end` range and localise the month name. */
function addDaysISO(iso: string, days: number): string {
  const d = new Date(`${iso}T00:00:00Z`);
  if (Number.isNaN(d.getTime())) return iso;
  d.setUTCDate(d.getUTCDate() + days);
  return d.toISOString().slice(0, 10);
}
export function formatPeriod(kind: string, period: string, locale: string): string {
  if (kind === 'weekly') {
    const end = addDaysISO(period, 6);
    return end === period ? period : `${period} → ${end}`;
  }
  if (kind === 'monthly') {
    const [y, m] = period.split('-').map(Number);
    if (y && m) {
      return new Date(Date.UTC(y, m - 1, 1)).toLocaleDateString(locale, {
        month: 'short', year: 'numeric', timeZone: 'UTC',
      });
    }
  }
  return period;
}

/* Rows per page, tuned per bucket so a page is a meaningful span:
 * ~a month of days, a quarter+ of weeks, a year of months. */
const ROWS_PER_PAGE: Record<string, number> = { daily: 30, weekly: 15, monthly: 12 };
export function rowsPerPage(kind: string): number {
  return ROWS_PER_PAGE[kind] ?? 30;
}

/* ── Model name → agent rollup ── */
const AGENT_COLORS: Record<string, string> = {
  claude: '#d4a574',
  codex: '#74b9a5',
  gemini: '#8fa9d4',
  other: 'var(--kr-text-muted)',
};
const AGENT_LABEL: Record<string, string> = {
  claude: 'Claude', codex: 'Codex', gemini: 'Gemini', other: 'Other',
};

function agentForModel(model: string): string {
  const m = model.toLowerCase();
  if (m.includes('claude') || m.includes('opus') || m.includes('sonnet') || m.includes('haiku')) return 'claude';
  if (m.includes('gpt') || m.includes('codex') || /^o[134]/.test(m)) return 'codex';
  if (m.includes('gemini')) return 'gemini';
  return 'other';
}

interface AgentTotal { agent: string; cost: number; tokens: number; }

function rollupByAgent(report: UsageReport): AgentTotal[] {
  const acc = new Map<string, AgentTotal>();
  for (const row of report.rows) {
    for (const mb of row.model_breakdowns) {
      const a = agentForModel(mb.model_name);
      const cur = acc.get(a) ?? { agent: a, cost: 0, tokens: 0 };
      cur.cost += mb.cost;
      cur.tokens += mb.total_tokens;
      acc.set(a, cur);
    }
  }
  return [...acc.values()].sort((x, y) => y.cost - x.cost);
}

interface UsageSectionProps {
  // Kept for call-site compatibility; unused (ccusage is global, not per-disc).
  onNavigateDiscussion?: (discussionId: string) => void;
}

export function UsageSection(_props: UsageSectionProps) {
  const { t, locale } = useT();
  const [period, setPeriod] = useState<Period>('daily');
  const [report, setReport] = useState<UsageReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showDetails, setShowDetails] = useState(false);
  const [page, setPage] = useState(0);

  const refresh = useCallback(async (p: Period) => {
    setLoading(true);
    setError(null);
    try {
      setReport(await usageApi.get(p));
    } catch (e) {
      setError(String(e));
      setReport(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { setPage(0); refresh(period); }, [period, refresh]);

  const byAgent = report ? rollupByAgent(report) : [];
  const totalAgentCost = byAgent.reduce((s, a) => s + a.cost, 0);
  const periods: Period[] = ['daily', 'weekly', 'monthly'];

  return (
    <div className="set-compression set-compression-state-ok" data-testid="usage-section">
      {/* ── Head : icon + title + period toggle (always visible) ── */}
      <div className="set-compression-head">
        <div className="set-compression-icon">
          <BarChart3 size={16} />
        </div>
        <div className="flex-1">
          <div className="flex-row gap-2" style={{ alignItems: 'center', flexWrap: 'wrap' }}>
            <span className="font-semibold text-base">{t('usage.title')}</span>
            <div className="flex-row gap-3" style={{ marginLeft: 'auto', alignItems: 'center' }}>
              <div className="set-usage-toggle-group" role="tablist" aria-label={t('usage.period')}>
                {periods.map(p => (
                  <button
                    key={p}
                    role="tab"
                    aria-selected={period === p}
                    className="set-usage-toggle-btn"
                    data-active={period === p}
                    onClick={() => setPeriod(p)}
                    data-testid={`usage-period-${p}`}
                  >
                    {t(`usage.period.${p}`)}
                  </button>
                ))}
              </div>
              <button
                className="set-action-btn"
                onClick={() => refresh(period)}
                disabled={loading}
                style={{ padding: '3px 8px' }}
                aria-label={t('usage.refresh')}
                data-testid="usage-refresh"
              >
                <RefreshCw size={11} className={loading ? 'set-spin' : ''} />
              </button>
            </div>
          </div>
          <p className="set-compression-explainer">{t('usage.intro')}</p>
        </div>
      </div>

      {/* ── Status line : total + tokens + agent chips + details toggle ── */}
      {error ? (
        <div className="set-compression-warning" data-testid="usage-error">
          {t('usage.unavailable')} — {error}
        </div>
      ) : report ? (
        <>
          <div className="set-compression-status">
            <span className="set-compression-dot" aria-hidden="true" />
            <span className="set-compression-status-text" data-testid="usage-total-cost">
              {fmtCost(report.totals.total_cost)}
            </span>
            <span className="set-compression-savings">
              · {fmtTokens(report.totals.total_tokens)} {t('usage.tokensTotal')}
            </span>
            {report.agents_detected.length > 0 && (
              <span className="flex-row gap-2" style={{ marginLeft: 8, flexWrap: 'wrap' }}>
                {report.agents_detected.map(a => (
                  <span key={a} className="set-usage-legend" data-testid={`usage-agent-${a}`}>
                    <span className="set-usage-dot" style={{ background: AGENT_COLORS[a] ?? AGENT_COLORS.other }} />
                    <span className="text-sm">{AGENT_LABEL[a] ?? a}</span>
                  </span>
                ))}
              </span>
            )}
            <button
              type="button"
              className="set-compression-details-toggle"
              onClick={() => setShowDetails(v => !v)}
              aria-expanded={showDetails}
              data-testid="usage-details-toggle"
            >
              {t('usage.detailsToggle')}
              {showDetails ? <ChevronUp size={10} /> : <ChevronDown size={10} />}
            </button>
          </div>

          {showDetails && (
            <div className="set-compression-details" style={{ display: 'block' }}>
              {/* Per-agent breakdown bar + legend */}
              {byAgent.length > 0 && totalAgentCost > 0 && (
                <div className="mb-8">
                  <div className="set-usage-subtitle">{t('usage.byAgent')}</div>
                  <div className="set-usage-bar-container">
                    {byAgent.filter(a => a.cost > 0).map(a => (
                      <div
                        key={a.agent}
                        className="set-usage-bar-segment"
                        style={{ width: `${Math.max(2, (a.cost / totalAgentCost) * 100)}%`, background: AGENT_COLORS[a.agent] ?? AGENT_COLORS.other }}
                        title={`${AGENT_LABEL[a.agent] ?? a.agent}: ${fmtCost(a.cost)} — ${fmtTokens(a.tokens)}`}
                      />
                    ))}
                  </div>
                  <div className="flex-wrap gap-6 mt-3">
                    {byAgent.filter(a => a.cost > 0).map(a => (
                      <div key={a.agent} className="set-usage-legend">
                        <span className="set-usage-dot" style={{ background: AGENT_COLORS[a.agent] ?? AGENT_COLORS.other }} />
                        <span className="text-sm text-primary">{AGENT_LABEL[a.agent] ?? a.agent}</span>
                        <span className="text-sm text-muted">{fmtCost(a.cost)}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {/* Recent rows (most-recent first, paginated) */}
              {report.rows.length === 0 ? (
                <p className="text-muted text-sm" data-testid="usage-empty">{t('usage.empty')}</p>
              ) : (() => {
                const ordered = [...report.rows].reverse();
                const perPage = rowsPerPage(report.period_kind);
                const pageCount = Math.max(1, Math.ceil(ordered.length / perPage));
                const safePage = Math.min(page, pageCount - 1);
                const pageRows = ordered.slice(safePage * perPage, safePage * perPage + perPage);
                return (
                  <>
                    <table className="usage-table" data-testid="usage-table">
                      <thead>
                        <tr>
                          <th style={{ textAlign: 'left' }}>{t('usage.col.period')}</th>
                          <th style={{ textAlign: 'right' }}>{t('usage.col.input')}</th>
                          <th style={{ textAlign: 'right' }}>{t('usage.col.output')}</th>
                          <th style={{ textAlign: 'right' }}>{t('usage.col.cache')}</th>
                          <th style={{ textAlign: 'right' }}>{t('usage.col.total')}</th>
                          <th style={{ textAlign: 'right' }}>{t('usage.col.cost')}</th>
                        </tr>
                      </thead>
                      <tbody>
                        {pageRows.map((r, i) => (
                          <tr key={`${r.period}-${i}`}>
                            <td>{formatPeriod(report.period_kind, r.period, locale)}</td>
                            <td style={{ textAlign: 'right' }}>{fmtTokens(r.input_tokens)}</td>
                            <td style={{ textAlign: 'right' }}>{fmtTokens(r.output_tokens)}</td>
                            <td style={{ textAlign: 'right' }} title={t('usage.cacheTooltip')}>
                              {fmtTokens(r.cache_creation_tokens + r.cache_read_tokens)}
                            </td>
                            <td style={{ textAlign: 'right' }}>{fmtTokens(r.total_tokens)}</td>
                            <td style={{ textAlign: 'right', fontWeight: 600 }}>{fmtCost(r.total_cost)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                    {pageCount > 1 && (
                      <div className="usage-pager" data-testid="usage-pager">
                        <button
                          type="button"
                          className="set-action-btn"
                          onClick={() => setPage(p => Math.max(0, p - 1))}
                          disabled={safePage === 0}
                          aria-label={t('usage.prevPage')}
                          data-testid="usage-prev-page"
                        >
                          <ChevronLeft size={12} />
                        </button>
                        <span className="usage-pager-label" data-testid="usage-page-indicator">
                          {t('usage.pageOf', safePage + 1, pageCount)}
                        </span>
                        <button
                          type="button"
                          className="set-action-btn"
                          onClick={() => setPage(p => Math.min(pageCount - 1, p + 1))}
                          disabled={safePage >= pageCount - 1}
                          aria-label={t('usage.nextPage')}
                          data-testid="usage-next-page"
                        >
                          <ChevronRight size={12} />
                        </button>
                      </div>
                    )}
                  </>
                );
              })()}
            </div>
          )}
        </>
      ) : (
        <div className="set-compression-status">
          <span className="set-compression-status-text" data-testid="usage-loading">{t('usage.loading')}</span>
        </div>
      )}

      {/* ── Footer : powered by ccusage ── */}
      <div className="set-compression-actions">
        <span className="set-compression-attrib">
          {t('usage.poweredBy')}{' '}
          <a href={CCUSAGE_GITHUB_URL} target="_blank" rel="noreferrer" className="set-compression-link">
            ccusage <ExternalLink size={10} />
          </a>
          {' '}({t('usage.openSource')})
        </span>
      </div>
    </div>
  );
}
