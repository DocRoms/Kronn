import { useCallback, useEffect, useRef, useState } from 'react';
import { ExternalLink, Loader2, Send } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { discussions as discussionsApi, projects as projectsApi } from '../lib/api';
import { AGENT_LABELS } from '../lib/constants';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import type { UnusableExternalAgentTarget } from '../lib/externalAgentIdentity';
import type {
  AgentType, ModelTiersConfig, SendMessageRequest, TdReviewItem, TdTalk,
} from '../types/generated';
import type { AgentSwitchTarget } from './AgentSwitchPicker';
import { AgentCompareSelector } from './AgentCompareSelector';
import { toMessageTargets, type CompareTarget } from '../lib/compareTargets';

const POLL_MS = 4000;

export interface TdTalkPanelProps {
  projectId: string;
  item: TdReviewItem;
  choices: AgentSwitchTarget[];
  unusable: UnusableExternalAgentTarget[];
  installedAgentTypes: AgentType[];
  targets: CompareTarget[];
  onTargetsChange: (targets: CompareTarget[] | null) => void;
  modelTiers?: ModelTiersConfig | null;
  onTalkChanged: () => void | Promise<void>;
  onOpenDiscussion: (discId: string) => void;
}

function targetLabel(target: CompareTarget, choices: AgentSwitchTarget[]): string {
  const choice = choices.find(c => c.agent === target.agent && (c.connectionId ?? undefined) === target.connection_id);
  return choice?.label ?? AGENT_LABELS[target.agent] ?? target.agent;
}

/**
 * "Discuter de cette TD": validation messages naming the sheet, then its own
 * discussion. Sending goes through the ordinary discussion machinery: the
 * sheet's discussion is created with the chosen agents, then run or written to.
 */
export function TdTalkPanel({
  projectId, item, choices, unusable, installedAgentTypes, targets, onTargetsChange, modelTiers,
  onTalkChanged, onOpenDiscussion,
}: TdTalkPanelProps) {
  const { t } = useT();
  const [talk, setTalk] = useState<TdTalk | null>(null);
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [talkError, setTalkError] = useState<string | null>(null);
  const mounted = useRef(true);

  const load = useCallback(async () => {
    try {
      const data = await projectsApi.tdTalk(projectId, item.id);
      if (mounted.current) setTalk(data);
    } catch {
      // The thread is a convenience: a failed read leaves the last one shown.
    }
  }, [projectId, item.id]);

  useEffect(() => {
    mounted.current = true;
    projectsApi.tdTalk(projectId, item.id)
      .then(data => { if (mounted.current) setTalk(data); })
      .catch(() => { /* the thread is a convenience: keep what is shown */ });
    return () => { mounted.current = false; };
  }, [projectId, item.id]);

  useEffect(() => {
    if (!sending) return;
    const timer = window.setInterval(() => { void load(); }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [sending, load]);

  const send = useAsyncGuard(async () => {
    const message = draft.trim();
    if (!message || targets.length === 0) return;
    const messageTargets = toMessageTargets(targets);
    setSending(true);
    const finish = () => {
      if (!mounted.current) return;
      setSending(false);
      void load();
      void onTalkChanged();
    };
    try {
      const start = await projectsApi.startTdTalk(projectId, item.id, { message, targets: messageTargets });
      setDraft('');
      await load();
      const fail = (error: string) => { console.warn('[td-talk]', error); finish(); };
      if (start.created) {
        await discussionsApi.runAgent(start.discussion_id, () => {}, finish, fail);
      } else {
        await discussionsApi.sendMessageStream(
          start.discussion_id,
          { content: message, targets: messageTargets } as SendMessageRequest,
          () => {},
          finish,
          fail,
        );
      }
    } catch (e) {
      setSending(false);
      setTalkError(e instanceof Error ? e.message : String(e));
    }
  });

  const names = targets.map(target => targetLabel(target, choices)).join(', ');

  return (
    <section className="td-talk" data-testid="td-talk">
      <h4>{t('techDebt.talk.title')}</h4>
      <AgentCompareSelector
        targets={targets}
        choices={choices}
        unusable={unusable}
        installedAgentTypes={installedAgentTypes}
        defaultTier="default"
        addAgent={choices[0]?.agent ?? 'ClaudeCode'}
        modelTiers={modelTiers}
        label={t('techDebt.talk.agents')}
        testIdPrefix="td-talk"
        onChange={onTargetsChange}
      />
      <div className="td-thread">
        {talk === null && <p className="td-empty">{t('techDebt.talk.loading')}</p>}
        {talk && talk.messages.length === 0 && <p className="td-empty">{t('techDebt.talk.empty')}</p>}
        {talk?.messages.map(message => (
          <article key={message.id} className="td-msg" data-role={message.role}>
            <header>
              <strong>
                {message.role === 'User'
                  ? (message.author ?? t('techDebt.talk.you'))
                  : (message.agent_type ? (AGENT_LABELS[message.agent_type] ?? message.agent_type) : message.role)}
              </strong>
              {message.source === 'validation' && (
                <span className="td-chip">{t('techDebt.talk.fromValidation')}</span>
              )}
              {message.excerpt && <span className="td-msg-excerpt">{t('techDebt.talk.excerpt')}</span>}
            </header>
            <p>{message.content}</p>
          </article>
        ))}
        {sending && (
          <p className="td-msg-pending"><Loader2 size={12} className="spin" /> {t('techDebt.talk.sending')}</p>
        )}
      </div>
      {talkError && <p className="td-block-error">{t('techDebt.talk.error', talkError)}</p>}
      <form
        className="td-talk-input"
        onSubmit={event => { event.preventDefault(); setTalkError(null); void send(); }}
      >
        <textarea
          value={draft}
          rows={2}
          maxLength={8000}
          onChange={event => setDraft(event.target.value)}
          placeholder={targets.length > 0
            ? t('techDebt.talk.placeholder', names)
            : t('techDebt.talk.placeholderNone')}
          aria-label={t('techDebt.talk.title')}
          onKeyDown={event => {
            if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void send();
            }
          }}
        />
        <button
          type="submit"
          className="td-primary-btn"
          disabled={sending || !draft.trim() || targets.length === 0}
          data-testid="td-talk-send"
        >
          <Send size={12} /> {t('techDebt.talk.send')}
        </button>
      </form>
      {talk?.discussion_id && (
        <button type="button" className="td-link-btn" onClick={() => { if (talk.discussion_id) onOpenDiscussion(talk.discussion_id); }}>
          <ExternalLink size={12} /> {t('techDebt.talk.openDiscussion')}
        </button>
      )}
    </section>
  );
}
