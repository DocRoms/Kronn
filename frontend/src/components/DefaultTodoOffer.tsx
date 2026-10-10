import { useEffect, useState } from 'react';
import { ListTodo } from 'lucide-react';
import type { DefaultTodoStatus } from '../types/generated';
import { pages as pagesApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { userError } from '../lib/userError';
import './DefaultTodoOffer.css';

export interface DefaultTodoOfferProps {
  /** Changes whenever the Page list does, so a deletion is noticed. */
  refreshKey: unknown;
  onInstalled: (pageId: string) => void;
}

/**
 * KT-1030 — Kronn's own todo board, offered again only when it is absent:
 * after its deletion, or when the user kept a todo page of their own.
 */
export function DefaultTodoOffer({ refreshKey, onInstalled }: DefaultTodoOfferProps) {
  const { t } = useT();
  const [status, setStatus] = useState<DefaultTodoStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    pagesApi.defaultTodo()
      .then(next => { if (active) setStatus(next); })
      .catch(() => { /* nothing to offer without a status */ });
    return () => { active = false; };
  }, [refreshKey]);

  const install = useAsyncGuard(async () => {
    try {
      const next = await pagesApi.installDefaultTodo();
      setStatus(next);
      setError(null);
      if (next.page_id) onInstalled(next.page_id);
    } catch (cause) {
      setError(userError(cause));
    }
  });

  if (!status || (status.state !== 'removed' && status.state !== 'kept_existing')) return null;
  const kept = status.state === 'kept_existing';
  return (
    <div className="default-todo-offer" data-testid="default-todo-offer">
      <ListTodo size={12} aria-hidden />
      <span>{t(kept ? 'pages.defaultTodo.kept' : 'pages.defaultTodo.removed')}</span>
      <button type="button" onClick={() => { void install(); }}>
        {t(kept ? 'pages.defaultTodo.installAlongside' : 'pages.defaultTodo.reinstall')}
      </button>
      {error && <p className="default-todo-offer__error" role="alert">{error}</p>}
    </div>
  );
}
