import { ArrowLeft, ArrowRight, BarChart3, Clock3, ExternalLink, Hash, Loader2, RefreshCw, Scale, X } from 'lucide-react';
import { useMemo, useState } from 'react';
import { MarkdownContent } from './MessageBubble';
import { AGENT_LABELS, MODEL_TIER_ICONS, agentColor, modelForAgentTier } from '../lib/constants';
import type { AgentType, Discussion, ModelTiersConfig } from '../types/generated';
import { BatchCompareDetailsPanel } from './BatchCompareDetailsPanel';
import './BatchComparePanel.css';

interface BatchComparePanelProps {
  runId: string;
  label: string;
  discussions: Discussion[];
  loading: boolean;
  error: string | null;
  modelTiers?: ModelTiersConfig | null;
  availableAgents: AgentType[];
  runningIds: ReadonlySet<string>;
  onRefresh: () => void;
  onOpenDiscussion: (discussionId: string) => void;
  onClose: () => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

function lastAgentAnswer(discussion: Discussion) {
  for (let index = discussion.messages.length - 1; index >= 0; index -= 1) {
    const message = discussion.messages[index];
    if (message.role === 'Agent') return message;
  }
  return null;
}

function lastSystemCause(discussion: Discussion) {
  for (let index = discussion.messages.length - 1; index >= 0; index -= 1) {
    const message = discussion.messages[index];
    if (message.role === 'System' && message.content.trim()) return message.content.trim();
  }
  return null;
}

function formatDuration(durationMs?: number | null) {
  if (durationMs == null) return null;
  const seconds = Math.max(0, durationMs / 1000);
  if (seconds < 60) return `${seconds < 10 ? seconds.toFixed(1) : Math.round(seconds)} s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes} min ${Math.round(seconds % 60)} s`;
}

function reconcileColumnOrder(previous: string[], discussions: Discussion[]) {
  const incomingIds = discussions.map(discussion => discussion.id);
  const incoming = new Set(incomingIds);
  return [
    ...previous.filter(id => incoming.has(id)),
    ...incomingIds.filter(id => !previous.includes(id)),
  ];
}

/** Dedicated comparison workspace: the batch remains the durable parent, while
 * every child discussion stays independently inspectable and resumable. */
export function BatchComparePanel({
  runId,
  label,
  discussions,
  loading,
  error,
  modelTiers,
  availableAgents,
  runningIds,
  onRefresh,
  onOpenDiscussion,
  onClose,
  t,
}: BatchComparePanelProps) {
  const [columnOrder, setColumnOrder] = useState<string[]>([]);
  const [showDetails, setShowDetails] = useState(false);

  // A refresh replaces the Discussion objects. Reconcile during render instead
  // of mirroring props through an effect: the stored state contains only the
  // user's moves, while vanished/new children are a pure projection.
  const orderedIds = useMemo(
    () => reconcileColumnOrder(columnOrder, discussions),
    [columnOrder, discussions],
  );

  const orderedDiscussions = useMemo(() => {
    const byId = new Map(discussions.map(discussion => [discussion.id, discussion]));
    return orderedIds.map(id => byId.get(id)).filter((discussion): discussion is Discussion => discussion != null);
  }, [orderedIds, discussions]);

  const moveColumn = (discussionId: string, offset: -1 | 1) => {
    setColumnOrder(previous => {
      const base = reconcileColumnOrder(previous, discussions);
      const from = base.indexOf(discussionId);
      const to = from + offset;
      if (from < 0 || to < 0 || to >= base.length) return base;
      const next = [...base];
      [next[from], next[to]] = [next[to], next[from]];
      return next;
    });
  };

  return (
    <section className="disc-compare-workspace" aria-labelledby="disc-compare-title">
      <header className="disc-compare-header">
        <div className="disc-compare-heading">
          <span className="disc-compare-icon" aria-hidden="true"><Scale size={18} /></span>
          <div>
            <h2 id="disc-compare-title">{t('disc.compare.title')}</h2>
            <p>{label} · {t('disc.compare.count', discussions.length)}</p>
          </div>
        </div>
        <div className="disc-compare-header-actions">
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => setShowDetails(value => !value)}
            aria-expanded={showDetails}
          >
            <BarChart3 size={14} />
            {t('disc.compare.details')}
          </button>
          <button type="button" className="btn btn-sm btn-ghost" onClick={onRefresh} disabled={loading}>
            <RefreshCw size={14} className={loading ? 'spin' : undefined} />
            {t('disc.compare.refresh')}
          </button>
          <button type="button" className="btn btn-ghost btn-icon" onClick={onClose} aria-label={t('common.close')}>
            <X size={17} />
          </button>
        </div>
      </header>

      {error && <div className="disc-compare-state" data-kind="error">{t('disc.compare.error', error)}</div>}
      {loading && discussions.length === 0 && (
        <div className="disc-compare-state"><Loader2 size={17} className="spin" />{t('disc.compare.loading')}</div>
      )}

      {orderedDiscussions.length > 0 && (
        <div className="disc-compare-columns" data-layout={orderedDiscussions.length === 2 ? 'split' : 'scroll'}>
          {orderedDiscussions.map((discussion, index) => {
            const answer = lastAgentAnswer(discussion);
            const running = runningIds.has(discussion.id) || discussion.awaiting_agent;
            const failureCause = !answer && !running ? lastSystemCause(discussion) : null;
            const tier = discussion.tier ?? 'default';
            const model = answer?.model
              || modelForAgentTier(discussion.agent, tier, modelTiers, t('disc.defaultAgentModel'));
            const agentLabel = AGENT_LABELS[discussion.agent] ?? discussion.agent;
            const duration = formatDuration(answer?.duration_ms);
            const tokens = answer?.tokens_used != null && answer.tokens_used > 0
              ? answer.tokens_used.toLocaleString()
              : null;
            return (
              <article key={discussion.id} className="disc-compare-column" data-running={running}>
                <header className="disc-compare-column-head">
                  <div className="disc-compare-target-name" style={{ color: agentColor(discussion.agent) }}>
                    <span className="disc-compare-index">{index + 1}</span>
                    <strong>{agentLabel}</strong>
                    <span className="disc-compare-tier" title={tier}>{MODEL_TIER_ICONS[tier]}</span>
                  </div>
                  <div className="disc-compare-reorder">
                    <button
                      type="button"
                      className="btn btn-xs btn-ghost btn-icon"
                      onClick={() => moveColumn(discussion.id, -1)}
                      disabled={index === 0}
                      aria-label={t('disc.compare.moveLeft', agentLabel)}
                      title={t('disc.compare.moveLeft', agentLabel)}
                    >
                      <ArrowLeft size={13} />
                    </button>
                    <button
                      type="button"
                      className="btn btn-xs btn-ghost btn-icon"
                      onClick={() => moveColumn(discussion.id, 1)}
                      disabled={index === orderedDiscussions.length - 1}
                      aria-label={t('disc.compare.moveRight', agentLabel)}
                      title={t('disc.compare.moveRight', agentLabel)}
                    >
                      <ArrowRight size={13} />
                    </button>
                  </div>
                  <span className="disc-compare-model" title={model}>{model}</span>
                  <div className="disc-compare-metrics">
                    <span title={duration == null ? t('disc.compare.metricUnavailable') : undefined}>
                      <Clock3 size={12} />
                      <strong>{duration ?? '—'}</strong>
                      <small>{t('disc.compare.duration')}</small>
                    </span>
                    <span title={tokens == null ? t('disc.compare.metricUnavailable') : undefined}>
                      <Hash size={12} />
                      <strong>{tokens ?? '—'}</strong>
                      <small>{t('disc.compare.tokens')}</small>
                    </span>
                  </div>
                  <div className="disc-compare-run-meta">
                    <span data-status={running ? 'running' : answer ? 'done' : 'waiting'}>
                      {running ? t('disc.compare.running') : answer ? t('disc.compare.done') : t('disc.compare.waiting')}
                    </span>
                  </div>
                  <button
                    type="button"
                    className="btn btn-xs btn-ghost disc-compare-open"
                    onClick={() => onOpenDiscussion(discussion.id)}
                  >
                    <ExternalLink size={13} /> {t('disc.compare.openDiscussion')}
                  </button>
                </header>
                <div className="disc-compare-answer">
                  {answer ? (
                    <MarkdownContent content={answer.content} discussionId={discussion.id} sourceMessageId={answer.id} />
                  ) : running ? (
                    <div className="disc-compare-wait"><Loader2 size={16} className="spin" />{t('disc.compare.generating')}</div>
                  ) : failureCause ? (
                    <div className="disc-compare-failure" role="alert">
                      <strong>{t('disc.compare.failureReason')}</strong>
                      <p>{failureCause}</p>
                    </div>
                  ) : (
                    <div className="disc-compare-wait">{t('disc.compare.noAnswer')}</div>
                  )}
                </div>
              </article>
            );
          })}
        </div>
      )}
      {showDetails && (
        <BatchCompareDetailsPanel
          runId={runId}
          discussions={discussions}
          availableAgents={availableAgents}
          modelTiers={modelTiers}
          onOpenDiscussion={onOpenDiscussion}
          onClose={() => setShowDetails(false)}
          t={t}
        />
      )}
    </section>
  );
}
