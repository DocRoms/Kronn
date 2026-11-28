import { AlertTriangle, Check, ChevronRight, Clock, Copy, Cpu, Loader2, Play, Square, X } from 'lucide-react';
import type { AgentType } from '../types/generated';
import type { QueuedMessage } from '../hooks/useMessageQueue';
import { AGENT_LABELS, agentColor, agentTextColor } from '../lib/constants';
import { useT } from '../lib/I18nContext';
import { useLiveRun, type ReplyIds } from '../lib/agentRunProgress';
import type { AgentIdentityModel, AgentIdentityRole } from './ui/agentIdentityKinds';
import { ActivityAgentIdentity } from './ActivityAgentIdentity';
import { AgentRunProgressPanel } from './AgentRunProgressPanel';
import { MarkdownContent } from './MessageBubble';
import { Message, MessageActions } from './Message';

/* KT-1902 — the messages of the discussion page that are not yet stored: a
 * reply waiting, refused or being written, a debate turn, the outbox. Each is
 * a variant of Message, not a bubble of its own. */

function StopReplyButton({ agentLabel, stopping, onStop }: { agentLabel: string; stopping: boolean; onStop: () => void }) {
  const { t } = useT();
  return (
    <button
      type="button"
      className="disc-pending-agent-stop"
      onClick={onStop}
      disabled={stopping}
      aria-label={t('disc.stopThisReply', agentLabel)}
      title={t('disc.stopThisReply', agentLabel)}
    >
      {stopping ? <Loader2 size={11} className="spin" /> : <Square size={10} fill="currentColor" />}
      {stopping ? t('disc.stoppingReply') : t('disc.stopReply')}
    </button>
  );
}

/** A reply Kronn owes and has not started writing: queued, starting, or
 *  refused (then it says why, as the backend wrote it). */
export function PendingAgentReplyMessage({
  replyId,
  discussionId,
  replyIds,
  agent,
  triggerMessageId,
  status,
  lastError,
  stopping,
  onStop,
  role,
  model,
}: {
  role?: AgentIdentityRole;
  model: AgentIdentityModel;
  replyId: string;
  discussionId: string;
  replyIds: ReplyIds;
  agent: AgentType;
  triggerMessageId: string;
  status: string;
  lastError?: string | null;
  stopping: boolean;
  onStop: () => void;
}) {
  const { t } = useT();
  const run = useLiveRun(discussionId, { id: replyId, agent, triggerMessageId }, replyIds);
  const identity = (
    <div className="disc-msg-agent-label" data-testid="activity-agent-identity">
      <ActivityAgentIdentity agent={agent} role={role} model={model} />
      {status !== 'Failed' && <Loader2 size={12} aria-hidden="true" className="spin" />}
    </div>
  );
  // A refused sibling used to vanish: the placeholder disappeared and the room
  // showed nothing, while Kronn had written down exactly why.
  if (status === 'Failed') {
    return (
      <Message
        variant="error"
        tone="failed"
        rowProps={{ 'data-reply-trigger': triggerMessageId, 'data-testid': `failed-agent-${agent}` }}
        header={identity}
      >
        <div className="disc-failed-agent">
          <AlertTriangle size={11} />
          <span>{t('disc.agentDidNotStart')}</span>
        </div>
        {/* The reason as the backend wrote it. Not translated: it names a
            connection, a model or a path, and a paraphrase would lose the
            part that tells you what to change. */}
        {lastError && <p className="disc-failed-agent-reason">{lastError}</p>}
      </Message>
    );
  }
  return (
    <Message
      variant="streaming"
      tone="pending"
      rowProps={{
        'data-reply-trigger': triggerMessageId,
        'data-testid': `pending-agent-${agent}`,
        'aria-live': run ? undefined : 'polite',
      }}
      header={identity}
    >
      <div className="disc-pending-agent-status">
        <div className="disc-streaming-waiting">
          <span className="disc-pulse-dot" />
          {status === 'Pending' ? t('disc.queued') : t('disc.running')}
        </div>
        <StopReplyButton agentLabel={AGENT_LABELS[agent] ?? agent} stopping={stopping} onStop={onStop} />
      </div>
      {run && <AgentRunProgressPanel run={run} agentLabel={AGENT_LABELS[agent] ?? agent} />}
    </Message>
  );
}

function formatElapsed(elapsed: number): string {
  return elapsed >= 60
    ? `${Math.floor(elapsed / 60)}m${String(elapsed % 60).padStart(2, '0')}s`
    : `${elapsed}s`;
}

/** The reply being written: its text so far, the elapsed time and Stop. It
 *  never folds while it streams. */
export function StreamingAgentReplyMessage({
  replyId,
  discussionId,
  replyIds,
  agent,
  triggerMessageId,
  elapsed,
  text,
  logs,
  showLogs,
  onToggleLogs,
  stopping,
  onStop,
  recovering,
  recoveryLabel,
  agentLabel,
  waitingUpstream,
  role,
  model,
}: {
  role?: AgentIdentityRole;
  model: AgentIdentityModel;
  replyId: string;
  discussionId: string;
  replyIds: ReplyIds;
  agent: AgentType;
  triggerMessageId: string;
  elapsed: number;
  text: string;
  logs: string[];
  showLogs: boolean;
  onToggleLogs: () => void;
  stopping: boolean;
  onStop: () => void;
  recovering: boolean;
  recoveryLabel: string | null;
  agentLabel?: string;
  /** Queued behind another run rather than working — worth saying so. */
  waitingUpstream: boolean;
}) {
  const { t } = useT();
  const displayAgent = agentLabel ?? AGENT_LABELS[agent] ?? agent;
  const run = useLiveRun(discussionId, { id: replyId, agent, triggerMessageId }, replyIds);
  return (
    <Message
      variant="streaming"
      tone="streaming"
      rowProps={{
        'data-reply-trigger': triggerMessageId,
        'data-testid': `streaming-agent-${agent}`,
        'aria-live': run ? undefined : 'polite',
      }}
      header={(
        <div className="disc-msg-agent-label" data-testid="activity-agent-identity">
          <ActivityAgentIdentity agent={agent} label={displayAgent} role={role} model={model} />
          {recovering
            ? <AlertTriangle size={10} aria-hidden="true" />
            : <Loader2 size={10} aria-hidden="true" className="spin" />}
        </div>
      )}
      headerActions={(
        <span className="disc-pending-agent-actions">
          <span className="disc-streaming-elapsed">{formatElapsed(elapsed)}</span>
          <StopReplyButton agentLabel={displayAgent} stopping={stopping} onStop={onStop} />
        </span>
      )}
    >
      {recoveryLabel && (
        <div className="disc-stream-recovery" role="status">
          <AlertTriangle size={12} />
          <span>{recoveryLabel}</span>
        </div>
      )}
      {text ? (
        <div className="disc-streaming-md">
          <MarkdownContent content={text} />
        </div>
      ) : run ? null : (
        <div className="disc-streaming-waiting" aria-live="assertive">
          <span className="disc-pulse-dot" />
          {waitingUpstream ? t('disc.waitingForSlot') : t('disc.running')}
          {logs.length > 0 && (
            <span className="disc-streaming-log-hint">— {logs.at(-1)?.slice(0, 60)}</span>
          )}
        </div>
      )}
      {run && <AgentRunProgressPanel run={run} agentLabel={displayAgent} />}
      {logs.length > 0 && (
        <div className="disc-streaming-logs">
          <button type="button" className="disc-logs-toggle" onClick={onToggleLogs} aria-expanded={showLogs}>
            <ChevronRight size={10} className="disc-chevron" data-expanded={showLogs} />
            {t('disc.logs')} ({logs.length})
          </button>
          {showLogs && (
            <div className="disc-logs-panel">
              {logs.map((log, index) => <div key={index}>{log}</div>)}
            </div>
          )}
        </div>
      )}
    </Message>
  );
}

/** Between « Launch the debate » and the first agent's first words: say it
 *  runs, with the last notes Kronn posted. */
export function DebateLaunchingMessage({ systemMessages }: { systemMessages: string[] }) {
  const { t } = useT();
  return (
    <Message variant="streaming" tone="debate" rowProps={{ 'aria-live': 'polite', 'data-testid': 'debate-launching' }}>
      <div className="disc-streaming-waiting">
        <span className="disc-pulse-dot" />
        {t('disc.debateLaunching')}
      </div>
      {systemMessages.length > 0 && (
        <ul className="disc-orch-systems">
          {systemMessages.slice(-5).map((message, index) => <li key={index}>{message}</li>)}
        </ul>
      )}
    </Message>
  );
}

/** One agent's turn in a debate, as it is written. */
export function DebateStreamMessage({
  agent,
  agentType,
  round,
  text,
  done,
}: {
  agent: string;
  agentType?: string;
  round: number | string;
  text: string;
  done: boolean;
}) {
  const { t } = useT();
  const colourOf = (agentType || agent) as AgentType;
  return (
    <Message
      variant="streaming"
      tone="debate"
      rowProps={{ 'data-testid': 'debate-stream' }}
      bubbleStyle={{ borderLeft: `3px solid ${agentColor(colourOf)}` }}
      header={(
        <div className="disc-orch-stream-agent" style={{ color: agentTextColor(colourOf) }}>
          <Cpu size={10} /> {agent}
          <span className="disc-orch-round">
            {round === 'synthesis' ? t('disc.synthesis') : t('disc.debateRound', round)}
          </span>
          {!done && <Loader2 size={9} className="spin" aria-hidden="true" />}
        </div>
      )}
    >
      {text ? (
        <MarkdownContent content={text} />
      ) : !done ? (
        <div className="disc-orch-thinking">{t('disc.thinking', agent)}</div>
      ) : (
        // Finished with no chunk — a CLI that crashed before printing or
        // returned empty. Without a word the bubble reads as a ghost.
        <div className="disc-orch-empty">
          <AlertTriangle size={11} aria-hidden="true" /> {t('disc.debateAgentEmpty', agent)}
        </div>
      )}
    </Message>
  );
}

/** The outbox: one bubble on the user's side, a line per queued part, all
 *  sent together as one message once the running reply ends. */
export function QueuedMessages({
  messages,
  copiedId,
  onCopy,
  onRetry,
  onRemove,
  onClearAll,
}: {
  messages: QueuedMessage[];
  copiedId?: string | null;
  onCopy: (id: string, content: string) => void;
  onRetry: (id: string) => void;
  onRemove: (id: string) => void;
  onClearAll: () => void;
}) {
  const { t } = useT();
  if (messages.length === 0) return null;
  return (
    <Message
      variant="queued"
      tone="queued"
      rowProps={{ role: 'group', 'aria-label': t('disc.queuedAria'), 'data-testid': 'queued-messages' }}
      header={(
        <span className="disc-queued-head" title={t('disc.queuedHint')}>
          <Clock size={11} className="disc-queued-clock" aria-hidden="true" />
          <span className="disc-queued-label">{t('disc.queuedAria')}</span>
        </span>
      )}
      headerActions={messages.length > 1 ? (
        <button
          type="button"
          className="disc-queued-clear"
          onClick={onClearAll}
          aria-label={t('disc.queuedClearAll')}
          title={t('disc.queuedClearAll')}
        >
          {t('disc.queuedClearAll')}
        </button>
      ) : undefined}
    >
      {messages.map(queued => (
        <div key={queued.id} className="disc-queued-line" data-status={queued.status}>
          {queued.targets?.map(target => (
            <span
              key={`${target.kind}:${target.agent_type}:${target.cli_session_id ?? ''}`}
              className="disc-queued-agent"
            >
              @{target.agent_type}
            </span>
          ))}
          {queued.replyToMessageId && (
            <span className="disc-queued-agent">
              {t('disc.reply')} #{queued.replyToMessageId.slice(0, 8)}
            </span>
          )}
          <span className="disc-queued-text">{queued.content}</span>
          <span className="disc-queued-status" title={queued.status === 'failed' ? queued.error ?? undefined : undefined}>
            {queued.status === 'sending' && <Loader2 size={10} className="spin" />}
            {t(`disc.queuedStatus.${queued.status}`)}
          </span>
          <MessageActions
            actions={{
              copy: {
                label: t('disc.copyMessage'),
                icon: copiedId === queued.id ? <Check /> : <Copy />,
                onSelect: () => onCopy(queued.id, queued.content),
                state: copiedId === queued.id ? 'copied' : undefined,
              },
              retry: queued.status === 'failed' ? {
                label: t('disc.queuedRetry'),
                icon: <Play />,
                onSelect: () => onRetry(queued.id),
                className: 'disc-queued-retry',
              } : undefined,
              delete: {
                label: t('disc.queuedCancel'),
                icon: <X />,
                onSelect: () => onRemove(queued.id),
                disabled: queued.status === 'sending',
                className: 'disc-queued-cancel',
              },
            }}
          />
        </div>
      ))}
    </Message>
  );
}
