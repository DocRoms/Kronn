// "Assistant conversations" of a plugin or an ApiCall step (KT-1111): the kept
// exchanges, reopened in the helper with "Resume" or deleted explicitly.
import { useCallback, useEffect, useState } from 'react';
import { History, Play, Trash2, Loader2 } from 'lucide-react';
import { assistantConversations, discussions as discussionsApi } from '../lib/api';
import { AGENT_LABELS } from '../lib/constants';
import { formatResourceDate } from '../lib/formatResourceDate';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import type { AssistantConversation, AssistantKind } from '../types/generated';
import {
  ASSISTANT_CONVERSATIONS_CHANGED,
  notifyAssistantConversationsChanged,
  proposalStatus,
} from './assistantConversation';

type Translator = (key: string, ...args: (string | number)[]) => string;

export interface AssistantConversationFilter {
  kind: AssistantKind;
  target_id?: string | null;
  unattached?: boolean;
  include_unattached?: boolean;
  pending_ids?: string[];
  unattached_step?: string | null;
  target_step?: string | null;
  plugin_id?: string | null;
}

interface AssistantConversationListProps {
  filter: AssistantConversationFilter;
  /** The conversation open in the helper right now, if any. */
  activeDiscussionId: string | null;
  onResume: (conversation: AssistantConversation) => void | Promise<void>;
  t: Translator;
}

export function AssistantConversationList({ filter, activeDiscussionId, onResume, t }: AssistantConversationListProps) {
  const [open, setOpen] = useState(false);
  const [rows, setRows] = useState<AssistantConversation[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const filterKey = JSON.stringify(filter);

  const refresh = useCallback(() => {
    const parsed = JSON.parse(filterKey) as AssistantConversationFilter;
    assistantConversations.list(parsed)
      .then(list => { setRows(list.filter(row => !row.archived)); setError(null); })
      .catch(e => setError(String(e)));
  }, [filterKey]);

  useEffect(() => {
    refresh();
    window.addEventListener(ASSISTANT_CONVERSATIONS_CHANGED, refresh);
    return () => window.removeEventListener(ASSISTANT_CONVERSATIONS_CHANGED, refresh);
  }, [refresh]);

  // Not awaited: opening another conversation must supersede a slow load,
  // which the helper's session guard then drops.
  const resume = useAsyncGuard(async (conversation: AssistantConversation) => {
    setOpen(false);
    void onResume(conversation);
  });

  const remove = useAsyncGuard(async (conversation: AssistantConversation) => {
    if (!window.confirm(t('aiHelper.conversations.deleteConfirm'))) return;
    try {
      await discussionsApi.delete(conversation.discussion_id);
      notifyAssistantConversationsChanged();
    } catch (e) {
      setError(String(e));
    }
  });

  const count = rows?.length ?? 0;
  if (count === 0 && !open) return null;

  return (
    <span className="ai-conv-wrap">
      <button
        type="button"
        className="ai-conv-toggle"
        onClick={() => setOpen(value => !value)}
        aria-expanded={open}
        title={t('aiHelper.conversations.hint')}
      >
        <History size={11} /> {t('aiHelper.conversations.title', count)}
      </button>
      {open && (
        <div className="ai-conv-panel" role="dialog" aria-label={t('aiHelper.conversations.panelTitle')}>
          <div className="ai-conv-panel-title">{t('aiHelper.conversations.panelTitle')}</div>
          {error && <div className="ai-conv-error" role="alert">{error}</div>}
          {rows === null && <Loader2 size={12} className="spin" />}
          {rows !== null && rows.length === 0 && (
            <div className="ai-conv-empty">{t('aiHelper.conversations.empty')}</div>
          )}
          <ul className="ai-conv-list">
            {(rows ?? []).map(row => {
              const status = proposalStatus(row);
              return (
                <li key={row.discussion_id} className="ai-conv-row" data-active={row.discussion_id === activeDiscussionId}>
                  <div className="ai-conv-meta">
                    <span className="ai-conv-date">{formatResourceDate(row.updated_at)}</span>
                    <span className="ai-conv-agent">{AGENT_LABELS[row.agent] ?? row.agent}</span>
                    <span className="ai-conv-status" data-status={status}>
                      {t(`aiHelper.conversations.status.${status}`)}
                    </span>
                  </div>
                  {row.target_label && <div className="ai-conv-label" title={row.title}>{row.target_label}</div>}
                  <div className="ai-conv-actions">
                    <button
                      type="button"
                      className="ai-conv-btn"
                      onClick={() => void resume(row)}
                      disabled={row.discussion_id === activeDiscussionId}
                    >
                      <Play size={10} /> {t('aiHelper.conversations.resume')}
                    </button>
                    <button
                      type="button"
                      className="ai-conv-btn ai-conv-btn-danger"
                      onClick={() => void remove(row)}
                      disabled={row.discussion_id === activeDiscussionId}
                      aria-label={t('aiHelper.conversations.delete')}
                      title={t('aiHelper.conversations.delete')}
                    >
                      <Trash2 size={10} />
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </span>
  );
}
