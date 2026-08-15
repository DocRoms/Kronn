import { ExternalLink, Loader2, RefreshCw, Scale, X } from 'lucide-react';
import { MarkdownContent } from './MessageBubble';
import { AGENT_LABELS, MODEL_TIER_ICONS, agentColor, modelForAgentTier } from '../lib/constants';
import type { Discussion, ModelTiersConfig } from '../types/generated';
import './BatchComparePanel.css';

interface BatchComparePanelProps {
  label: string;
  discussions: Discussion[];
  loading: boolean;
  error: string | null;
  modelTiers?: ModelTiersConfig | null;
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

/** Dedicated comparison workspace: the batch remains the durable parent, while
 * every child discussion stays independently inspectable and resumable. */
export function BatchComparePanel({
  label,
  discussions,
  loading,
  error,
  modelTiers,
  runningIds,
  onRefresh,
  onOpenDiscussion,
  onClose,
  t,
}: BatchComparePanelProps) {
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

      {discussions.length > 0 && (
        <div className="disc-compare-columns" data-layout={discussions.length === 2 ? 'split' : 'scroll'}>
          {discussions.map((discussion, index) => {
            const answer = lastAgentAnswer(discussion);
            const running = runningIds.has(discussion.id) || discussion.awaiting_agent;
            const tier = discussion.tier ?? 'default';
            const model = answer?.model
              || modelForAgentTier(discussion.agent, tier, modelTiers, t('disc.defaultAgentModel'));
            return (
              <article key={discussion.id} className="disc-compare-column" data-running={running}>
                <header className="disc-compare-column-head">
                  <div className="disc-compare-target-name" style={{ color: agentColor(discussion.agent) }}>
                    <span className="disc-compare-index">{index + 1}</span>
                    <strong>{AGENT_LABELS[discussion.agent] ?? discussion.agent}</strong>
                    <span className="disc-compare-tier" title={tier}>{MODEL_TIER_ICONS[tier]}</span>
                  </div>
                  <span className="disc-compare-model" title={model}>{model}</span>
                  <div className="disc-compare-run-meta">
                    <span data-status={running ? 'running' : answer ? 'done' : 'waiting'}>
                      {running ? t('disc.compare.running') : answer ? t('disc.compare.done') : t('disc.compare.waiting')}
                    </span>
                    {answer?.duration_ms != null && <span>{Math.round(answer.duration_ms / 1000)} s</span>}
                    {answer?.tokens_used != null && answer.tokens_used > 0 && <span>{answer.tokens_used.toLocaleString()} tokens</span>}
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
                  ) : (
                    <div className="disc-compare-wait">{t('disc.compare.noAnswer')}</div>
                  )}
                </div>
              </article>
            );
          })}
        </div>
      )}
    </section>
  );
}
