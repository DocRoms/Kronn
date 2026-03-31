import { useState } from 'react';
import { stats as statsApi } from '../../lib/api';
import { useApi } from '../../hooks/useApi';
import { useT } from '../../lib/I18nContext';
import type { TokenUsageSummary, DailyUsage } from '../../types/generated';
import {
  TrendingUp, RefreshCw, ChevronRight, DollarSign, Hash,
} from 'lucide-react';
import '../../pages/SettingsPage.css';

/* ── Formatters ── */

function fmtCost(usd: number | null | undefined): string {
  if (usd == null || usd === 0) return '$0.00';
  if (usd < 0.01) return '< $0.01';
  return `$${usd.toFixed(2)}`;
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function fmtDate(iso: string): string {
  const d = new Date(iso + 'T00:00:00');
  return `${d.getDate()}/${d.getMonth() + 1}`;
}

/* ── Constants ── */

const PROVIDER_COLORS: Record<string, string> = {
  Anthropic: '#d4a574',
  OpenAI: '#74b9a5',
  Google: '#8fa9d4',
  Mistral: '#d48f74',
  Amazon: '#c4a3d4',
};

const PROVIDER_FIELDS: { key: keyof DailyUsage; provider: string }[] = [
  { key: 'anthropic', provider: 'Anthropic' },
  { key: 'openai', provider: 'OpenAI' },
  { key: 'google', provider: 'Google' },
  { key: 'mistral', provider: 'Mistral' },
  { key: 'amazon', provider: 'Amazon' },
];

type ViewMode = 'tokens' | 'cost';
type FilterMode = 'all' | 'discussions' | 'workflows';

interface UsageSectionProps {
  onNavigateDiscussion?: (discussionId: string) => void;
}

export function UsageSection({ onNavigateDiscussion }: UsageSectionProps) {
  const { t } = useT();
  const { data, loading, refetch } = useApi<TokenUsageSummary>(() => statsApi.tokenUsage(), []);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<ViewMode>('cost');
  const [filter, setFilter] = useState<FilterMode>('all');

  const toggleExpand = (key: string) => setExpanded(expanded === key ? null : key);

  return (
    <div id="settings-usage" className="set-card">
      <div className="set-section">
        {/* ── Header with controls ── */}
        <div className="flex-row gap-4 set-section-header-lg" style={{ flexWrap: 'wrap' }}>
          <TrendingUp size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('config.usage')}</span>

          <div className="flex-row gap-3" style={{ marginLeft: 'auto' }}>
            {/* View mode toggle: tokens / cost */}
            <div className="set-usage-toggle-group">
              <button
                className="set-usage-toggle-btn"
                data-active={viewMode === 'tokens'}
                onClick={() => setViewMode('tokens')}
                title="Tokens"
              >
                <Hash size={11} />
              </button>
              <button
                className="set-usage-toggle-btn"
                data-active={viewMode === 'cost'}
                onClick={() => setViewMode('cost')}
                title="Cost"
              >
                <DollarSign size={11} />
              </button>
            </div>

            {/* Filter: all / discussions / workflows */}
            <div className="set-usage-toggle-group">
              {(['all', 'discussions', 'workflows'] as FilterMode[]).map(f => (
                <button
                  key={f}
                  className="set-usage-toggle-btn"
                  data-active={filter === f}
                  onClick={() => setFilter(f)}
                >
                  {f === 'all' ? t('config.usageFilterAll') : f === 'discussions' ? t('config.usageFilterDisc') : t('config.usageFilterWf')}
                </button>
              ))}
            </div>

            <button
              className="set-action-btn"
              onClick={refetch}
              disabled={loading}
              style={{ padding: '3px 8px' }}
            >
              <RefreshCw size={11} className={loading ? 'set-spin' : ''} />
            </button>
          </div>
        </div>

        {!data && !loading && (
          <p className="text-muted text-sm">{t('config.usageEmpty')}</p>
        )}

        {data && (
          <>
            {/* ── Summary cards ── */}
            <div className="set-usage-cards">
              <SummaryCard
                label={t('config.usageTotalTokens')}
                value={fmtTokens(
                  filter === 'workflows' ? data.workflow_tokens
                    : filter === 'discussions' ? data.discussion_tokens
                    : data.total_tokens
                )}
                sub={viewMode === 'cost' ? fmtCost(data.total_cost_usd) : undefined}
                accent
              />
              {filter !== 'workflows' && (
                <SummaryCard
                  label={t('config.usageDiscussions')}
                  value={viewMode === 'cost' ? fmtCost(
                    data.by_provider.reduce((s, p) => s + (p.cost_usd ?? 0), 0) * (data.discussion_tokens / Math.max(data.total_tokens, 1))
                  ) : fmtTokens(data.discussion_tokens)}
                />
              )}
              {filter !== 'discussions' && (
                <SummaryCard
                  label={t('config.usageWorkflows')}
                  value={viewMode === 'cost' ? fmtCost(
                    data.by_provider.reduce((s, p) => s + (p.cost_usd ?? 0), 0) * (data.workflow_tokens / Math.max(data.total_tokens, 1))
                  ) : fmtTokens(data.workflow_tokens)}
                />
              )}
              {data.by_provider.length > 0 && data.by_provider
                .filter(p => p.tokens_used > 0)
                .sort((a, b) => b.tokens_used - a.tokens_used)
                .slice(0, 3)
                .map(p => (
                  <SummaryCard
                    key={p.provider}
                    label={p.provider}
                    value={viewMode === 'cost' ? fmtCost(p.cost_usd) : fmtTokens(p.tokens_used)}
                    color={PROVIDER_COLORS[p.provider]}
                  />
                ))
              }
            </div>

            {/* ── Daily history chart ── */}
            {data.daily_history.length > 0 && (
              <div className="mb-8">
                <div className="set-usage-subtitle">{t('config.usageDaily')}</div>
                <DailyChart
                  days={data.daily_history}
                  viewMode={viewMode}
                />
              </div>
            )}

            {/* ── Provider bar ── */}
            {data.by_provider.length > 0 && (
              <div className="mb-8">
                <div className="set-usage-subtitle">{t('config.usageByProvider')}</div>
                <div className="set-usage-bar-container">
                  {data.by_provider
                    .filter(p => p.tokens_used > 0)
                    .sort((a, b) => b.tokens_used - a.tokens_used)
                    .map(p => {
                      const total = viewMode === 'cost'
                        ? data.by_provider.reduce((s, x) => s + (x.cost_usd ?? 0), 0)
                        : data.total_tokens;
                      const val = viewMode === 'cost' ? (p.cost_usd ?? 0) : p.tokens_used;
                      const pct = total > 0 ? Math.max(2, (val / total) * 100) : 0;
                      return (
                        <div
                          key={p.provider}
                          className="set-usage-bar-segment"
                          style={{ width: `${pct}%`, background: PROVIDER_COLORS[p.provider] ?? 'var(--kr-text-muted)' }}
                          title={`${p.provider}: ${fmtTokens(p.tokens_used)} — ${fmtCost(p.cost_usd)}`}
                        />
                      );
                    })}
                </div>
                <div className="flex-wrap gap-6 mt-3">
                  {data.by_provider
                    .filter(p => p.tokens_used > 0)
                    .sort((a, b) => b.tokens_used - a.tokens_used)
                    .map(p => (
                      <div key={p.provider} className="set-usage-legend">
                        <span className="set-usage-dot" style={{ background: PROVIDER_COLORS[p.provider] ?? 'var(--kr-text-muted)' }} />
                        <span className="text-sm text-primary">{p.provider}</span>
                        <span className="text-sm text-muted">
                          {viewMode === 'cost' ? fmtCost(p.cost_usd) : fmtTokens(p.tokens_used)}
                        </span>
                      </div>
                    ))}
                </div>
              </div>
            )}

            {/* ── By project ── */}
            {data.by_project.length > 0 && (
              <div className="mb-8">
                <div className="set-usage-subtitle">{t('config.usageByProject')}</div>
                {(() => {
                  const maxVal = Math.max(...data.by_project.map(p => viewMode === 'cost' ? p.cost_usd : p.tokens_used));
                  return data.by_project.slice(0, 8).map(p => {
                    const val = viewMode === 'cost' ? p.cost_usd : p.tokens_used;
                    const pct = maxVal > 0 ? (val / maxVal) * 100 : 0;
                    return (
                      <div key={p.project_id} className="set-usage-project-row">
                        <div className="set-usage-project-label">
                          <span className="text-sm text-primary">{p.project_name}</span>
                          <span className="text-sm text-dim">
                            {viewMode === 'cost' ? fmtCost(p.cost_usd) : fmtTokens(p.tokens_used)}
                          </span>
                        </div>
                        <div className="set-usage-project-bar-bg">
                          <div
                            className="set-usage-project-bar-fill"
                            style={{ width: `${pct}%` }}
                          />
                        </div>
                      </div>
                    );
                  });
                })()}
              </div>
            )}

            {/* ── Top discussions ── */}
            {filter !== 'workflows' && data.top_discussions.length > 0 && (
              <div className="mb-8">
                <button
                  className="set-usage-subtitle set-usage-collapse-btn"
                  onClick={() => toggleExpand('disc')}
                >
                  <ChevronRight
                    size={12}
                    style={{
                      transform: expanded === 'disc' ? 'rotate(90deg)' : 'none',
                      transition: 'transform 0.15s',
                    }}
                  />
                  {t('config.usageTopDiscussions')}
                  <span className="set-usage-count">{data.top_discussions.length}</span>
                </button>
                {expanded === 'disc' && data.top_discussions.map(d => (
                  <div key={d.id} className="set-usage-row">
                    {onNavigateDiscussion ? (
                      <button
                        className="set-usage-row-link"
                        onClick={() => onNavigateDiscussion(d.id)}
                        title={d.name}
                      >
                        {d.name}
                      </button>
                    ) : (
                      <span className="text-sm text-primary set-usage-row-name">{d.name}</span>
                    )}
                    <span className="text-sm text-muted">{fmtTokens(d.tokens_used)}</span>
                    <span className="text-sm text-dim" style={{ minWidth: 60, textAlign: 'right' }}>
                      {fmtCost(d.cost_usd)}
                    </span>
                  </div>
                ))}
              </div>
            )}

            {/* ── Top workflows ── */}
            {filter !== 'discussions' && data.top_workflows.length > 0 && (
              <div className="mb-8">
                <button
                  className="set-usage-subtitle set-usage-collapse-btn"
                  onClick={() => toggleExpand('wf')}
                >
                  <ChevronRight
                    size={12}
                    style={{
                      transform: expanded === 'wf' ? 'rotate(90deg)' : 'none',
                      transition: 'transform 0.15s',
                    }}
                  />
                  {t('config.usageTopWorkflows')}
                  <span className="set-usage-count">{data.top_workflows.length}</span>
                </button>
                {expanded === 'wf' && data.top_workflows.map(w => (
                  <div key={w.id} className="set-usage-row">
                    <span className="text-sm text-primary set-usage-row-name">{w.name}</span>
                    <span className="text-sm text-muted">{fmtTokens(w.tokens_used)}</span>
                    <span className="text-sm text-dim" style={{ minWidth: 60, textAlign: 'right' }}>
                      {fmtCost(w.cost_usd)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}

/* ── Summary card sub-component ── */

function SummaryCard({ label, value, sub, accent, color }: {
  label: string;
  value: string;
  sub?: string;
  accent?: boolean;
  color?: string;
}) {
  return (
    <div className="set-usage-card" style={color ? { borderTopColor: color } : undefined}>
      <div
        className="set-usage-card-value"
        style={accent ? { color: 'var(--kr-accent)' } : color ? { color } : undefined}
      >
        {value}
      </div>
      <div className="set-usage-card-label">{label}</div>
      {sub && <div className="set-usage-card-sub">{sub}</div>}
    </div>
  );
}

/* ── Daily bar chart (pure CSS) ── */

function DailyChart({ days, viewMode }: { days: DailyUsage[]; viewMode: ViewMode }) {
  const maxVal = Math.max(...days.map(d => viewMode === 'cost' ? d.cost_usd : d.tokens));
  if (maxVal === 0) return null;

  return (
    <div className="set-usage-chart">
      {days.map(day => {
        const total = viewMode === 'cost' ? day.cost_usd : day.tokens;
        const heightPct = (total / maxVal) * 100;

        // Build stacked segments per provider
        const segments: { color: string; pct: number }[] = [];
        if (total > 0) {
          for (const { key, provider } of PROVIDER_FIELDS) {
            const v = day[key] as number;
            if (v > 0) {
              // For cost view, approximate provider share from token ratio
              const share = viewMode === 'cost' ? (v / day.tokens) * day.cost_usd : v;
              segments.push({
                color: PROVIDER_COLORS[provider] ?? 'var(--kr-text-muted)',
                pct: (share / total) * 100,
              });
            }
          }
        }

        return (
          <div key={day.date} className="set-usage-chart-col" title={`${day.date}: ${viewMode === 'cost' ? fmtCost(day.cost_usd) : fmtTokens(day.tokens)}`}>
            <div className="set-usage-chart-bar" style={{ height: `${Math.max(heightPct, 2)}%` }}>
              {segments.map((seg, i) => (
                <div
                  key={i}
                  className="set-usage-chart-seg"
                  style={{ height: `${seg.pct}%`, background: seg.color }}
                />
              ))}
            </div>
            <div className="set-usage-chart-label">{fmtDate(day.date)}</div>
          </div>
        );
      })}
    </div>
  );
}
