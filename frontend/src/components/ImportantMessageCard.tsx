// KT-619 — the steering card, and the bar that finds them.
//
// The card renders the DURABLE row, never the fence it was written as: the
// fence is what the orchestrator typed, the row is what Kronn recorded and
// what the counter, the filter and the navigation all agree on.

import { useCallback, useMemo, useState } from 'react';
import { AlertTriangle, ChevronDown, ChevronUp } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { IMPORTANT_CATEGORIES, useImportantMessages } from '../lib/importantMessages';
import type { ImportantCategory } from '../types/generated';
import './ImportantMessageCard.css';

function formatDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

export interface ImportantMessageCardProps {
  discussionId?: string;
  sourceMessageId?: string;
}

/**
 * Renders the card recorded for this message.
 *
 * A fence with no row means it was refused or malformed. Saying so beats
 * rendering raw JSON at the reader, and beats rendering nothing at all.
 */
export function ImportantMessageCard({ discussionId, sourceMessageId }: ImportantMessageCardProps) {
  const { t } = useT();
  const { items, loaded } = useImportantMessages(discussionId);
  const card = useMemo(
    () => items.find((item) => item.message_id === sourceMessageId),
    [items, sourceMessageId],
  );

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
}

/**
 * Counter, filter and previous/next.
 *
 * Navigation scrolls the transcript to a card; it never re-renders or reorders
 * the list, so the reader's position is the only thing that moves — and only
 * when they asked for it.
 */
export function ImportantMessagesBar({ discussionId }: ImportantMessagesBarProps) {
  const { t } = useT();
  const { items, totalAll, loaded } = useImportantMessages(discussionId);
  const [category, setCategory] = useState<ImportantCategory | ''>('');
  const [cursor, setCursor] = useState(0);

  const visible = useMemo(
    () => (category ? items.filter((item) => item.category === category) : items),
    [items, category],
  );

  // A filter change makes the old index meaningless, so the reset happens in
  // the event that caused it — not in an effect watching for it afterwards.
  const changeCategory = (next: ImportantCategory | '') => {
    setCategory(next);
    setCursor(0);
  };

  const go = useCallback(
    (delta: number) => {
      if (visible.length === 0) return;
      const next = Math.min(Math.max(cursor + delta, 0), visible.length - 1);
      setCursor(next);
      const target = visible[next];
      const node = document.querySelector(`[data-message-id="${CSS.escape(target.message_id)}"]`);
      // Scrolling is the whole effect: no state in the transcript changes, so
      // nothing the reader had open collapses.
      node?.scrollIntoView({ behavior: 'smooth', block: 'center' });
      (node as HTMLElement | null)?.focus?.({ preventScroll: true });
    },
    [cursor, visible],
  );

  if (!loaded || totalAll === 0) return null;

  return (
    <div className="disc-important-bar" role="group" aria-label={t('disc.important.barLabel')}>
      <span className="disc-important-count">
        {t('disc.important.count', String(totalAll))}
      </span>

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
        onClick={() => go(-1)}
        disabled={visible.length === 0 || cursor === 0}
        aria-label={t('disc.important.previous')}
      >
        <ChevronUp size={16} aria-hidden="true" />
      </button>
      <button
        type="button"
        onClick={() => go(1)}
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
