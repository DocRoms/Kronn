import { useCallback, useEffect, useRef, useState } from 'react';
import { pages as pagesApi } from '../lib/api';
import type { LivePageAction } from '../types/generated';
import type { LivePageActionIntent } from '../lib/live-page-sandbox';

export interface LivePageActiveActionState {
  activation: number;
  actionRef: string;
  bindings: Record<string, string>;
  anchor: { left: number; top: number; width: number; height: number };
  /** What this click turned into once launched or declined — its own launch,
   * never written back over the offer the other buttons still draw from. */
  card?: LivePageAction;
}

export interface UseLivePageActionsResult {
  actions: LivePageAction[];
  activeAction: LivePageActiveActionState | null;
  selectedAction: LivePageAction | null;
  handleIntent: (intent: LivePageActionIntent) => void;
  handleChanged: (action: LivePageAction, activation: number) => void;
  reload: (pageId: string | null) => Promise<void>;
}

/**
 * Owns the load -> validate -> activate -> mutate lifecycle for a Page's
 * inline Kronn actions, shared by the embedded viewer, the standalone tab
 * and every mosaic tile so a `kronn:page-action` intent resolves identically
 * everywhere. Validation reads a ref mirror, not the `actions` state, because
 * the sandbox bridge relay is connected once per iframe load and would
 * otherwise close over a stale snapshot.
 *
 * `actions` are the Page's offers and are never mutated by a click: one block
 * draws a button per dataset row, so the result of one row's launch belongs to
 * that click's card alone. Writing it into the list is what made every other
 * row reopen on the first row's result.
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

  // Keyed on the activation, not the block: a launch that answers after the
  // user has already clicked another row of the same block must not land on
  // that row's card.
  const handleChanged = useCallback((changed: LivePageAction, activation: number) => {
    setActiveAction(current => current && current.activation === activation
      ? { ...current, card: changed }
      : current);
  }, []);

  const selectedAction = activeAction
    ? activeAction.card
      ?? actions.find(action => action.action_ref === activeAction.actionRef)
      ?? null
    : null;

  return { actions, activeAction, selectedAction, handleIntent, handleChanged, reload };
}
