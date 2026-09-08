// KT-619 — the shared source of truth for a discussion's steering cards.
//
// Lives outside the component file so the card, the bar and any future reader
// hold the SAME rows: a counter and a list that fetch separately are a counter
// and a list that eventually disagree.

import { useCallback, useEffect, useSyncExternalStore } from 'react';
import { discussions as discussionsApi } from './api';
import type { ImportantCategory, ImportantMessage } from '../types/generated';

export const IMPORTANT_CATEGORIES: ImportantCategory[] = [
  'decision',
  'scope_change',
  'dod_waiver',
  'blocking_alert',
  'human_action_required',
  'accepted_delivery',
];

interface Snapshot {
  items: ImportantMessage[];
  totalAll: number;
  loaded: boolean;
}

const EMPTY: Snapshot = { items: [], totalAll: 0, loaded: false };

// One store per discussion: the card in the transcript and the bar above it
// must not disagree about how many cards exist, so they read the same rows.
const stores = new Map<string, Snapshot>();
const listeners = new Map<string, Set<() => void>>();
const inFlight = new Map<string, Promise<void>>();

function emit(discussionId: string) {
  listeners.get(discussionId)?.forEach((listener) => listener());
}

function subscribe(discussionId: string, listener: () => void) {
  const set = listeners.get(discussionId) ?? new Set();
  set.add(listener);
  listeners.set(discussionId, set);
  return () => {
    set.delete(listener);
    if (set.size === 0) listeners.delete(discussionId);
  };
}

async function load(discussionId: string): Promise<void> {
  const running = inFlight.get(discussionId);
  if (running) return running;
  const task = (async () => {
    try {
      const list = await discussionsApi.importantMessages(discussionId);
      stores.set(discussionId, {
        items: list.items,
        totalAll: list.total_all,
        loaded: true,
      });
    } catch {
      // A failed load leaves the transcript readable and the bar hidden; the
      // cards are an overlay on the thread, never a precondition for it.
      stores.set(discussionId, { ...EMPTY, loaded: true });
    } finally {
      inFlight.delete(discussionId);
      emit(discussionId);
    }
  })();
  inFlight.set(discussionId, task);
  return task;
}

/** Drop what we hold, without fetching. Mounted consumers keep their last
 *  render until something asks for a reload. */
export function clearImportantMessages(discussionId: string) {
  stores.delete(discussionId);
}

/** Invalidate and reload, after an append that may have published a card. */
export function refreshImportantMessages(discussionId: string) {
  clearImportantMessages(discussionId);
  void load(discussionId);
}

export function useImportantMessages(discussionId: string | undefined): Snapshot {
  const snapshot = useSyncExternalStore(
    useCallback(
      (listener: () => void) => (discussionId ? subscribe(discussionId, listener) : () => {}),
      [discussionId],
    ),
    () => (discussionId ? stores.get(discussionId) ?? EMPTY : EMPTY),
    () => EMPTY,
  );
  useEffect(() => {
    if (discussionId && !stores.has(discussionId)) void load(discussionId);
  }, [discussionId]);
  return snapshot;
}

