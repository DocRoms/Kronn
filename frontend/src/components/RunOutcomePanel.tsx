import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, CheckCircle2, ExternalLink, Loader2, MessageSquare, X } from 'lucide-react';
import { runsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { AGENT_LABELS } from '../lib/constants';
import type { RunOutcome, RunOutcomeAgentStatus } from '../types/generated';
import { MarkdownContent } from './MessageBubble';
import './RunOutcomePanel.css';

const REFRESH_MS = 4_000;

function statusIcon(status: RunOutcomeAgentStatus) {
  switch (status) {
    case 'working': return <Loader2 size={12} className="spin" aria-hidden />;
    case 'answered': return <CheckCircle2 size={12} aria-hidden />;
    case 'failed': return <AlertTriangle size={12} aria-hidden />;
    case 'cancelled': return <X size={12} aria-hidden />;
    default: return <MessageSquare size={12} aria-hidden />;
  }
}

/** What a launch produced, read from wherever the work actually happened: a
 * workflow's own result lists its steps, but the answer a human came for is
 * often in the discussion one of those steps opened. Shared by every action
 * card, so a Page and a Discussion tell the same story. Key it on its source:
 * a new run must never show the previous run's account, even briefly. */
export function RunOutcomePanel({
  runId,
  discussionId,
  runActive,
  onOpenDiscussion,
}: {
  runId?: string | null;
  discussionId?: string | null;
  /** The run itself is still going: new discussions may still appear. */
  runActive: boolean;
  onOpenDiscussion: (discussionId: string) => void;
}) {
  const { t } = useT();
  const [outcome, setOutcome] = useState<RunOutcome | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const agentWorking = outcome?.discussions.some(discussion => discussion.agent_status === 'working') ?? false;

  // Reading once per source, and following on a timer, are separate effects:
  // a change in whether to follow must not cost an immediate extra read.
  const read = useCallback(() => {
    const fetchOutcome = runId
      ? () => runsApi.outcome(runId)
      : discussionId ? () => runsApi.discussionOutcome(discussionId) : null;
    if (!fetchOutcome) return () => undefined;
    let cancelled = false;
    Promise.resolve()
      .then(fetchOutcome)
      .then(next => { if (!cancelled) { setOutcome(next); setUnavailable(false); } })
      .catch(() => { if (!cancelled) setUnavailable(true); });
    return () => { cancelled = true; };
  }, [discussionId, runId]);

  useEffect(() => read(), [read]);

  const keepFollowing = runActive || agentWorking;
  useEffect(() => {
    if (!keepFollowing) return undefined;
    let cancelPending: () => void = () => undefined;
    const timer = window.setInterval(() => {
      cancelPending();
      cancelPending = read();
    }, REFRESH_MS);
    return () => {
      window.clearInterval(timer);
      cancelPending();
    };
  }, [keepFollowing, read]);

  // Without the summary the reader must still reach the result.
  if (unavailable && !outcome) {
    return discussionId ? (
      <div className="run-outcome" data-testid="run-outcome">
        <button type="button" className="run-outcome__open" data-testid="run-outcome-open" onClick={() => onOpenDiscussion(discussionId)}>
          <ExternalLink size={12} aria-hidden /> {t('disc.action.openDiscussion')}
        </button>
      </div>
    ) : null;
  }
  if (!outcome || outcome.discussions.length === 0) return null;

  const hidden = outcome.discussion_count - outcome.discussions.length;
  return (
    <section className="run-outcome" data-testid="run-outcome" aria-label={t('disc.action.outcome.title')}>
      <h4 className="run-outcome__title">{t('disc.action.outcome.title')}</h4>
      <ul className="run-outcome__list">
        {outcome.discussions.map(discussion => (
          <li key={discussion.id} className="run-outcome__item" data-status={discussion.agent_status} data-testid="run-outcome-item">
            <div className="run-outcome__head">
              <span className="run-outcome__name">{discussion.title}</span>
              <span className="run-outcome__agent">{AGENT_LABELS[discussion.agent] ?? discussion.agent}</span>
              <span className="run-outcome__status" data-status={discussion.agent_status}>
                {statusIcon(discussion.agent_status)} {t(`disc.action.outcome.${discussion.agent_status}`)}
              </span>
            </div>
            {discussion.answer_excerpt && (
              <div className="run-outcome__excerpt" data-testid="run-outcome-excerpt">
                <MarkdownContent content={discussion.answer_excerpt} discussionId={discussion.id} />
                {discussion.answer_truncated && <span className="run-outcome__more">…</span>}
              </div>
            )}
            {discussion.diagnostic && <p className="run-outcome__diagnostic" role="alert">{discussion.diagnostic}</p>}
            <button type="button" className="run-outcome__open" data-testid="run-outcome-open" onClick={() => onOpenDiscussion(discussion.id)}>
              <ExternalLink size={12} aria-hidden /> {t('disc.action.openDiscussion')}
            </button>
          </li>
        ))}
      </ul>
      {hidden > 0 && <p className="run-outcome__hidden">{t('disc.action.outcome.more', hidden)}</p>}
    </section>
  );
}
