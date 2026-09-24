import { useCallback, useEffect, useState } from 'react';
import { ChevronDown, ChevronUp, RotateCcw, X } from 'lucide-react';
import { ChatInput } from './ChatInput';
import { agents as agentsApi, discussions as discussionsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { publishMessageSendSettled } from '../lib/messageSendLifecycle';
import { clearSubmittedDraft } from '../lib/chat-drafts';
import { useMessageQueue, type QueuedMessage, type QueuedMessageControl } from '../hooks/useMessageQueue';
import { userError } from '../lib/userError';
import { sendToDiscussion } from '../lib/sendToDiscussion';
import type { ToastFn } from '../hooks/useToast';
import type { AgentDetection, Discussion, MessageChannel, MessageTarget } from '../types/generated';

export function DiscussionMosaicComposer({ discussionId, title, toast }: {
  discussionId: string | null;
  title: string;
  toast: ToastFn;
}) {
  const { t } = useT();
  const [open, setOpen] = useState(true);
  const [agents, setAgents] = useState<AgentDetection[]>([]);
  // Tagged with the id it was loaded for, so a late answer for another tile is ignored.
  const [loaded, setLoaded] = useState<{ id: string; discussion: Discussion | null } | null>(null);

  useEffect(() => {
    agentsApi.detect().then(setAgents).catch(() => setAgents([]));
  }, []);

  useEffect(() => {
    if (!discussionId || !open) return;
    let current = true;
    discussionsApi.get(discussionId)
      .then(discussion => { if (current) setLoaded({ id: discussionId, discussion }); })
      .catch(() => { if (current) setLoaded({ id: discussionId, discussion: null }); });
    return () => { current = false; };
  }, [discussionId, open]);

  // Only the selected discussion's own record may back the input: a stale one
  // would send the draft to the previous tile.
  const forSelection = loaded && loaded.id === discussionId ? loaded : null;
  const ready = forSelection?.discussion?.id === discussionId ? forSelection.discussion : null;
  const loadError = forSelection !== null && forSelection.discussion === null;

  return <section className="discussion-mosaic-composer" data-open={open} aria-label={t('disc.mosaic.composer')}>
    <button type="button" className="discussion-mosaic-composer-toggle" aria-expanded={open} onClick={() => setOpen(value => !value)}>
      {open ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
      {discussionId ? t('disc.mosaic.replyIn', title) : t('disc.mosaic.selectToReply')}
    </button>
    {open && (!discussionId
      ? <p className="discussion-mosaic-composer-hint" role="status">{t('disc.mosaic.selectToReply')}</p>
      : loadError ? <p className="discussion-mosaic-composer-hint" role="alert">{t('disc.mosaic.unavailable')}</p>
        : !ready ? <p className="discussion-mosaic-composer-hint" role="status">{t('disc.mosaic.loading')}</p>
          : <BoundComposer key={ready.id} discussion={ready} agents={agents} title={title} toast={toast} />)}
  </section>;
}

/** Settles an accepted send. The input may have been collapsed or left before
 *  the receipt, with nobody listening: the stored draft is then still the
 *  submitted snapshot and would come back. Anything typed since is kept. */
function settleAccepted(discussionId: string, text: string) {
  publishMessageSendSettled(discussionId, text, 'accepted');
  clearSubmittedDraft(discussionId, text);
}

/** One discussion's input. Keyed by its id, so everything it sends is bound to
 *  that discussion even if the selection changes meanwhile. */
function BoundComposer({ discussion, agents, title, toast }: {
  discussion: Discussion;
  agents: AgentDetection[];
  title: string;
  toast: ToastFn;
}) {
  const { t } = useT();
  const id = discussion.id;
  const persist = useCallback(async (discId: string, queued: QueuedMessage, control: QueuedMessageControl) => {
    if (!control.beginPersist()) return;
    await sendToDiscussion(discId, queued.content, queued.targets, queued.targetAll ?? false, queued.replyToMessageId, 'main', queued.id);
    toast(t('disc.mosaic.sent', title), 'success');
  }, [t, title, toast]);
  // Same durable outbox as the discussion page: the entry and its UUID reach
  // localStorage first, then retries reuse that UUID until the server accepts.
  const { queue, enqueue, removeQueued, retryQueued } = useMessageQueue({ discId: id, onPersist: persist });

  const onSend = (text: string, targets?: MessageTarget[], targetAll = false, replyTo?: string, channel: MessageChannel = 'main') => {
    if (channel === 'note') {
      // Notes never wake an agent, so they skip the dispatch outbox.
      sendToDiscussion(id, text, undefined, false, replyTo, 'note')
        .then(() => settleAccepted(id, text))
        .catch(error => {
          publishMessageSendSettled(id, text, 'refused');
          toast(userError(error), 'error');
        });
      return;
    }
    if (enqueue(text, targets, targetAll, replyTo)) {
      settleAccepted(id, text);
    } else {
      publishMessageSendSettled(id, text, 'refused');
      toast(t('disc.queuedPersistFailed'), 'error');
    }
  };

  return <>
    {queue.length > 0 && <ul className="discussion-mosaic-outbox" aria-label={t('disc.mosaic.outbox')}>
      {queue.map(message => <li key={message.id} data-status={message.status}>
        <span>{message.content}</span>
        {message.status === 'failed'
          ? <>
            <small role="alert">{t('disc.mosaic.sendFailed')}</small>
            <button type="button" onClick={() => retryQueued(message.id)} aria-label={t('disc.mosaic.retry')} title={t('disc.mosaic.retry')}><RotateCcw size={13} /></button>
            <button type="button" onClick={() => removeQueued(message.id)} aria-label={t('disc.mosaic.discard')} title={t('disc.mosaic.discard')}><X size={13} /></button>
          </>
          : <small role="status">{t('disc.mosaic.sending')}</small>}
      </li>)}
    </ul>}
    <ChatInput
      discussion={discussion}
      agents={agents}
      sending={false}
      disabled={false}
      ttsEnabled={false}
      ttsState="idle"
      worktreeError={null}
      availableSkills={[]}
      availableDirectives={[]}
      onSend={onSend}
      onStop={() => undefined}
      onOrchestrate={() => undefined}
      onTtsToggle={() => undefined}
      onWorktreeErrorDismiss={() => undefined}
      onWorktreeRetry={() => undefined}
      isAgentRestricted={() => false}
      showVoiceControls={false}
      showDebate={false}
      toast={toast}
      t={t}
    />
  </>;
}
