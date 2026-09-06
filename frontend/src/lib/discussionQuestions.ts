// KT-595 — one read of a room's arbitration questions, shared by every card.
//
// A room can hold several questions, and each one renders from a fence inside
// its own message. Letting each card fetch for itself would issue one request
// per card for a list that is the same list every time — and the endpoint is
// polled, so that cost repeats. The cards read this store instead; it fetches
// once per discussion and tells them all when the answer changes.
import { useCallback, useSyncExternalStore } from 'react';
import { discussions as discussionsApi } from './api';
import { userError } from './userError';
import type { DiscussionQuestion } from '../types/generated';

export interface QuestionsSnapshot {
  questions: DiscussionQuestion[];
  pendingCount: number;
  /** No answer read yet for this room — not "the room has none". */
  loading: boolean;
  error: string;
}

const EMPTY: QuestionsSnapshot = {
  questions: [],
  pendingCount: 0,
  loading: true,
  error: '',
};

const snapshots = new Map<string, QuestionsSnapshot>();
const listeners = new Map<string, Set<() => void>>();
const inFlight = new Set<string>();

function publish(discussionId: string, next: QuestionsSnapshot) {
  snapshots.set(discussionId, next);
  listeners.get(discussionId)?.forEach(listener => listener());
}

/** Read the room's questions again. Safe to call from anywhere: a fetch
 *  already running for this room is joined rather than duplicated. */
export function refreshDiscussionQuestions(discussionId: string) {
  if (inFlight.has(discussionId)) return;
  inFlight.add(discussionId);
  discussionsApi.questions(discussionId)
    .then(list => publish(discussionId, {
      questions: list.questions,
      pendingCount: list.pending_count,
      loading: false,
      error: '',
    }))
    .catch(cause => publish(discussionId, {
      ...(snapshots.get(discussionId) ?? EMPTY),
      loading: false,
      error: userError(cause),
    }))
    .finally(() => { inFlight.delete(discussionId); });
}

export function useDiscussionQuestions(discussionId: string | null): QuestionsSnapshot {
  const subscribe = useCallback((listener: () => void) => {
    if (!discussionId) return () => {};
    const forRoom = listeners.get(discussionId) ?? new Set();
    forRoom.add(listener);
    listeners.set(discussionId, forRoom);
    // The first subscriber for a room it has never read triggers the read.
    // Later ones join whatever is already there.
    if (!snapshots.has(discussionId)) {
      snapshots.set(discussionId, EMPTY);
      refreshDiscussionQuestions(discussionId);
    }
    return () => {
      forRoom.delete(listener);
      if (forRoom.size === 0) listeners.delete(discussionId);
    };
  }, [discussionId]);

  const getSnapshot = useCallback(
    // The same object identity until something actually changes: returning a
    // fresh literal here would re-render every card on every render.
    () => (discussionId ? snapshots.get(discussionId) ?? EMPTY : EMPTY),
    [discussionId],
  );

  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** Replace one question after it was answered, without a round trip: the
 *  endpoint returns the updated row, and the card that posted it already has
 *  the answer on screen. */
export function applyAnsweredQuestion(discussionId: string, answered: DiscussionQuestion) {
  const current = snapshots.get(discussionId) ?? EMPTY;
  const questions = current.questions.some(question => question.id === answered.id)
    ? current.questions.map(question => (question.id === answered.id ? answered : question))
    : [...current.questions, answered];
  publish(discussionId, {
    questions,
    pendingCount: questions.filter(question => question.state === 'pending').length,
    loading: false,
    error: '',
  });
}

/** Test seam: the store outlives a component, so a suite that renders two
 *  discussions in a row must be able to start from nothing. */
export function resetDiscussionQuestions() {
  snapshots.clear();
  listeners.clear();
  inFlight.clear();
}
