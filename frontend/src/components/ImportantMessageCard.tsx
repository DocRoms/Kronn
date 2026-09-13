// KT-619 — the steering card, and the bar that finds them.
//
// The card renders the DURABLE row, never the fence it was written as: the
// fence is what the orchestrator typed, the row is what Kronn recorded and
// what the counter, the filter and the navigation all agree on.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, ChevronDown, ChevronUp } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import {
  IMPORTANT_CATEGORIES,
  refreshImportantMessages,
  useImportantMessages,
} from '../lib/importantMessages';
import type { ImportantCategory } from '../types/generated';
import './ImportantMessageCard.css';

function formatDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

export interface ImportantMessageCardProps {
  discussionId?: string;
  sourceMessageId?: string;
  /** Server events have no transport fence; absence is ordinary, not refusal. */
  serverEventOnly?: boolean;
}

/**
 * Renders the card recorded for this message.
 *
 * A fence with no row means it was refused or malformed. Saying so beats
 * rendering raw JSON at the reader, and beats rendering nothing at all.
 */
export function ImportantMessageCard({ discussionId, sourceMessageId, serverEventOnly = false }: ImportantMessageCardProps) {
  const { t } = useT();
  const { items, loaded } = useImportantMessages(discussionId);
  const card = useMemo(
    () => items.find((item) => item.message_id === sourceMessageId),
    [items, sourceMessageId],
  );

  // A display label or ordinary delivery notification is not proof that a
  // steering card exists. Only the durable row and its source kind decide.
  if (serverEventOnly && card?.source_kind !== 'orchestration') return null;

  if (!card) {
    if (!loaded) return <div className="disc-important-card" aria-busy="true" />;
    return (
      <div className="disc-important-card" data-category="blocking_alert" role="note">
        <div className="disc-important-head">
          <AlertTriangle size={16} aria-hidden="true" />
          <p className="disc-important-title">{t('disc.important.missingTitle')}</p>
        </div>
        <p className="disc-important-value">{t('disc.important.missingHint')}</p>
      </div>
    );
  }

  const references = Object.entries(card.references).filter(([, value]) => Boolean(value));

  return (
    <article
      className="disc-important-card"
      data-category={card.category}
      id={`important-${card.id}`}
      aria-label={t('disc.important.cardLabel', t(`disc.important.category.${card.category}`))}
    >
      <div className="disc-important-head">
        <span className="disc-important-category">
          {t(`disc.important.category.${card.category}`)}
        </span>
        <h3 className="disc-important-title">{card.title}</h3>
        <span className="disc-important-meta">
          {t(`disc.important.author.${card.author_kind}`)} · {card.author_label} ·{' '}
          {formatDate(card.created_at)}
        </span>
      </div>

      <p className="disc-important-highlight">{card.highlight}</p>

      <div className="disc-important-body">
        {card.context && (
          <div>
            <span className="disc-important-label">{t('disc.important.context')}</span>
            <span className="disc-important-value">{card.context}</span>
          </div>
        )}
        <div>
          <span className="disc-important-label">{t('disc.important.impact')}</span>
          <span className="disc-important-value">{card.impact}</span>
        </div>
        <div className="disc-important-action">
          <span className="disc-important-label">{t('disc.important.action')}</span>
          <span className="disc-important-value">
            {card.action_required.required
              ? [
                  card.action_required.action,
                  card.action_required.owner,
                  card.action_required.due,
                ]
                  .filter(Boolean)
                  .join(' · ')
              : t('disc.important.actionNone')}
          </span>
        </div>
        {references.length > 0 && (
          <ul className="disc-important-refs">
            {references.map(([key, value]) => (
              <li key={key}>
                {t(`disc.important.ref.${key}`)}: {value}
              </li>
            ))}
          </ul>
        )}
      </div>
    </article>
  );
}

export interface ImportantMessagesBarProps {
  discussionId?: string;
  /** Id of the newest durable message. Changing it means the transcript grew,
   *  which is the only moment a new card can have appeared. Same idiom as
   *  `DiscussionQuestionBanner`, and one fetch per arrival — not per bubble. */
  messageRevision?: string;
  /** The transcript's own navigation. Reused rather than reimplemented so a
   *  jump releases stick-to-bottom and highlights the target the way every
   *  other jump in this page does. */
  onNavigate?: (messageId: string) => void;
}

/**
 * Counter, filter and previous/next.
 *
 * Navigation scrolls the transcript to a card; it never re-renders or reorders
 * the list, so the reader's position is the only thing that moves — and only
 * when they asked for it.
 */
export function ImportantMessagesBar({
  discussionId,
  messageRevision,
  onNavigate,
}: ImportantMessagesBarProps) {
  const { t } = useT();
  const { items, totalAll, loaded } = useImportantMessages(discussionId);
  const [selections, setSelections] = useState<Record<string, {
    category: ImportantCategory | '';
    messageId?: string;
  }>>({});
  const room = discussionId ?? '';
  const selection = selections[room];
  const category = selection?.category ?? '';
  const previousRefresh = useRef<{ discussionId?: string; messageRevision?: string }>({});

  const visible = useMemo(
    () => (category ? items.filter((item) => item.category === category) : items),
    [items, category],
  );
  // Row identities survive re-fetching, insertion and room changes; numeric
  // positions do not. A removed selection falls back within the current list
  // without causing an unsolicited transcript jump.
  const cursor = Math.max(0, visible.findIndex(item => item.message_id === selection?.messageId));

  // A filter change makes the old index meaningless, so the reset happens in
  // the event that caused it — not in an effect watching for it afterwards.
  const changeCategory = (next: ImportantCategory | '') => {
    setSelections(current => ({ ...current, [room]: { category: next } }));
  };

  // A new durable message is the only moment a card can have appeared. Without
  // this the store stayed on its first GET, so an arriving card showed as
  // "not recorded" and the counter stayed behind until a reload.
  useEffect(() => {
    const newArrival = previousRefresh.current.discussionId === discussionId
      && previousRefresh.current.messageRevision !== messageRevision;
    previousRefresh.current = { discussionId, messageRevision };
    // Initial/new-room reads coalesce with the hook's mount request. A later
    // arrival in this room must invalidate even an already pending snapshot.
    if (discussionId && messageRevision) refreshImportantMessages(discussionId, newArrival);
  }, [discussionId, messageRevision]);

  const goTo = useCallback(
    (index: number) => {
      const target = visible[index];
      if (!target) return;
      setSelections(current => ({ ...current, [room]: { category, messageId: target.message_id } }));
      onNavigate?.(target.message_id);
    },
    [visible, onNavigate, room, category],
  );

  // Separate from `goTo` so the current card stays reachable when there is only
  // one: both arrows are disabled then, and without this nothing would reach it.
  const goToCurrent = useCallback(() => goTo(Math.min(cursor, visible.length - 1)), [
    goTo,
    cursor,
    visible.length,
  ]);

  if (!loaded || totalAll === 0) return null;

  return (
    <div className="disc-important-bar" role="group" aria-label={t('disc.important.barLabel')}>
      <button
        type="button"
        className="disc-important-count"
        onClick={goToCurrent}
        disabled={visible.length === 0}
        aria-label={t('disc.important.goToCurrent')}
      >
        {t('disc.important.count', String(totalAll))}
      </button>

      <label>
        <span className="sr-only">{t('disc.important.filterLabel')}</span>
        <select
          value={category}
          onChange={(event) => changeCategory(event.target.value as ImportantCategory | '')}
          aria-label={t('disc.important.filterLabel')}
        >
          <option value="">{t('disc.important.filterAll')}</option>
          {IMPORTANT_CATEGORIES.map((name) => (
            <option key={name} value={name}>
              {t(`disc.important.category.${name}`)}
            </option>
          ))}
        </select>
      </label>

      <button
        type="button"
        onClick={() => goTo(cursor - 1)}
        disabled={visible.length === 0 || cursor === 0}
        aria-label={t('disc.important.previous')}
      >
        <ChevronUp size={16} aria-hidden="true" />
      </button>
      <button
        type="button"
        onClick={() => goTo(cursor + 1)}
        disabled={visible.length === 0 || cursor >= visible.length - 1}
        aria-label={t('disc.important.next')}
      >
        <ChevronDown size={16} aria-hidden="true" />
      </button>

      <span className="disc-important-position" aria-live="polite">
        {visible.length === 0
          ? t('disc.important.noneInFilter')
          : t('disc.important.position', String(cursor + 1), String(visible.length))}
      </span>
    </div>
  );
}
