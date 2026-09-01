import { useCallback, useEffect, useRef, useState } from 'react';
import { pages as pagesApi } from '../lib/api';
import type { LivePageAction } from '../types/generated';
import type { LivePageActionIntent } from '../lib/live-page-sandbox';

export interface LivePageActiveActionState {
  activation: number;
  actionRef: string;
  bindings: Record<string, string>;
  anchor: { left: number; top: number; width: number; height: number };
}

export interface UseLivePageActionsResult {
  actions: LivePageAction[];
  activeAction: LivePageActiveActionState | null;
  selectedAction: LivePageAction | null;
  handleIntent: (intent: LivePageActionIntent) => void;
  handleChanged: (action: LivePageAction) => void;
  reload: (pageId: string | null) => Promise<void>;
}

/**
 * Owns the load -> validate -> activate -> mutate lifecycle for a Page's
 * inline Kronn actions, shared by the embedded viewer, the standalone tab
 * and every mosaic tile so a `kronn:page-action` intent resolves identically
 * everywhere. Validation reads a ref mirror, not the `actions` state, because
 * the sandbox bridge relay is connected once per iframe load and would
 * otherwise close over a stale snapshot.
 */
export function useLivePageActions(onUnavailable: () => void): UseLivePageActionsResult {
  const [actions, setActions] = useState<LivePageAction[]>([]);
  const actionsRef = useRef<LivePageAction[]>([]);
  const [activeAction, setActiveAction] = useState<LivePageActiveActionState | null>(null);
  const activationRef = useRef(0);
  const onUnavailableRef = useRef(onUnavailable);
  useEffect(() => { onUnavailableRef.current = onUnavailable; });

  const reload = useCallback(async (pageId: string | null) => {
    const next = pageId ? await pagesApi.actions(pageId) : [];
    setActions(next);
    actionsRef.current = next;
    setActiveAction(null);
  }, []);

  const handleIntent = useCallback((intent: LivePageActionIntent) => {
    const exists = actionsRef.current.some(action => action.action_ref === intent.actionRef);
    if (!exists) {
      onUnavailableRef.current();
      return;
    }
    activationRef.current += 1;
    setActiveAction({ ...intent, activation: activationRef.current });
  }, []);

  const handleChanged = useCallback((changed: LivePageAction) => {
    setActions(current => {
      const next = current.map(action => action.id === changed.id ? changed : action);
      actionsRef.current = next;
      return next;
    });
  }, []);

  const selectedAction = activeAction
    ? actions.find(action => action.action_ref === activeAction.actionRef) ?? null
    : null;

  return { actions, activeAction, selectedAction, handleIntent, handleChanged, reload };
}
