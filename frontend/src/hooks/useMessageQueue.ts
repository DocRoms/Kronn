import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { MessageTarget } from '../types/generated';

export type QueuedMessageStatus = 'queued' | 'preparing' | 'sending' | 'failed';

/** Ephemeral preparation controls; never part of the durable outbox entry. */
export interface QueuedMessageControl {
  signal: AbortSignal;
  /** Claim the write synchronously, only after preparation is still valid. */
  beginPersist: () => boolean;
}

/** A durable outbox entry created while another reply is already running. */
export interface QueuedMessage {
  /** Stable UUID reused as the backend's client_message_id on every retry. */
  id: string;
  content: string;
  targets?: MessageTarget[];
  targetAll?: boolean;
  replyToMessageId?: string;
  status: QueuedMessageStatus;
  attempts: number;
  createdAt: string;
  retryAt?: number;
  error?: string;
}

const OUTBOX_KEY_PREFIX = 'kronn:message-outbox:';
const RETRY_BASE_MS = 2_000;
const RETRY_MAX_MS = 30_000;

function nextId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  const bytes = new Uint8Array(16);
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(bytes);
  } else {
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Math.floor(Math.random() * 256);
    }
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function storageKey(discId: string): string {
  return `${OUTBOX_KEY_PREFIX}${discId}`;
}

function loadQueue(discId: string | null): QueuedMessage[] {
  if (!discId || typeof localStorage === 'undefined') return [];
  try {
    const raw = localStorage.getItem(storageKey(discId));
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap((value): QueuedMessage[] => {
      if (!value || typeof value !== 'object') return [];
      const candidate = value as Partial<QueuedMessage>;
      if (typeof candidate.id !== 'string' || typeof candidate.content !== 'string') return [];
      // `sending` means the old tab died before observing a receipt. Retry the
      // same UUID: the backend either inserts it or returns duplicate=true.
      const restoredStatus = candidate.status === 'failed' ? 'failed' : 'queued';
      return [{
        id: candidate.id,
        content: candidate.content,
        targets: candidate.targets,
        targetAll: candidate.targetAll,
        replyToMessageId: candidate.replyToMessageId,
        status: restoredStatus,
        attempts: Number.isFinite(candidate.attempts) ? Math.max(0, candidate.attempts ?? 0) : 0,
        createdAt: candidate.createdAt ?? new Date().toISOString(),
        retryAt: restoredStatus === 'failed' ? candidate.retryAt : undefined,
        error: restoredStatus === 'failed' ? candidate.error : undefined,
      }];
    });
  } catch {
    return [];
  }
}

function saveQueue(discId: string, queue: QueuedMessage[]): boolean {
  if (typeof localStorage === 'undefined') return false;
  try {
    if (queue.length === 0) localStorage.removeItem(storageKey(discId));
    else localStorage.setItem(storageKey(discId), JSON.stringify(queue));
    return true;
  } catch {
    return false;
  }
}

/**
 * Durable client outbox for follow-up messages.
 *
 * Every entry reaches localStorage before `enqueue` reports success. The pump
 * then asks the backend to commit the User row and a Pending dispatch using
 * the entry UUID as its idempotency key. A network/backend cut leaves the
 * entry visible as failed and retries the same identity with bounded backoff;
 * an accepted or duplicate receipt removes it exactly once.
 */
export function useMessageQueue({
  discId,
  onPersist,
  prepareBeforePersist = false,
}: {
  discId: string | null;
  onPersist: (discId: string, message: QueuedMessage, control: QueuedMessageControl) => Promise<void>;
  prepareBeforePersist?: boolean;
}) {
  const [queueState, setQueueState] = useState(() => ({
    discId,
    messages: loadQueue(discId),
  }));
  const [retryTick, setRetryTick] = useState(0);
  const restoredQueue = useMemo(() => loadQueue(discId), [discId]);
  const queue = queueState.discId === discId ? queueState.messages : restoredQueue;
  const queueRef = useRef(queue);
  const activeDiscRef = useRef(discId);
  const processingDiscsRef = useRef(new Map<string, {
    id: string;
    controller: AbortController;
    preparing: boolean;
  }>());
  const onPersistRef = useRef(onPersist);

  useLayoutEffect(() => {
    onPersistRef.current = onPersist;
  }, [onPersist]);

  const publishActiveQueue = useCallback((forDisc: string, next: QueuedMessage[]) => {
    if (!saveQueue(forDisc, next)) return false;
    if (activeDiscRef.current === forDisc) {
      queueRef.current = next;
      setQueueState({ discId: forDisc, messages: next });
    }
    return true;
  }, []);

  useLayoutEffect(() => {
    activeDiscRef.current = discId;
    queueRef.current = queue;
  }, [discId, queue]);

  // A proof is scoped to the open room, unlike a write already sent to the
  // backend. Preserve unsent text on navigation/reload, but never finish an old
  // preparation after leaving. Reopening obtains fresh authority and proof.
  useLayoutEffect(() => () => {
    if (!discId) return;
    const pending = processingDiscsRef.current.get(discId);
    if (!pending?.preparing) return;
    pending.controller.abort();
    saveQueue(discId, loadQueue(discId).map(message => message.id === pending.id
      ? { ...message, status: 'queued', retryAt: undefined, error: undefined }
      : message));
  }, [discId]);

  // Wake the pump when the earliest failed entry becomes retryable. The
  // stable UUID makes repeated network attempts safe even when the previous
  // acceptance receipt was lost in transit.
  useEffect(() => {
    const retryAt = queue
      .filter(message => message.status === 'failed' && message.retryAt != null)
      .reduce<number | null>((earliest, message) => {
        const messageRetryAt = message.retryAt;
        if (messageRetryAt == null) return earliest;
        return earliest == null ? messageRetryAt : Math.min(earliest, messageRetryAt);
      }, null);
    if (retryAt == null) return;
    const timeout = window.setTimeout(
      () => setRetryTick(current => current + 1),
      Math.max(0, retryAt - Date.now()),
    );
    return () => window.clearTimeout(timeout);
  }, [queue]);

  useEffect(() => {
    if (!discId || processingDiscsRef.current.has(discId)) return;
    const now = Date.now();
    const candidate = queue.find(message => (
      message.status === 'queued'
      || (message.status === 'failed' && message.retryAt != null && message.retryAt <= now)
    ));
    if (!candidate) return;

    const pending = {
      id: candidate.id,
      controller: new AbortController(),
      preparing: prepareBeforePersist,
    };
    processingDiscsRef.current.set(discId, pending);
    const sending = queue.map(message => message.id === candidate.id ? {
      ...message,
      status: prepareBeforePersist ? 'preparing' as const : 'sending' as const,
      attempts: message.attempts + 1,
      retryAt: undefined,
      error: undefined,
    } : message);
    if (!publishActiveQueue(discId, sending)) {
      processingDiscsRef.current.delete(discId);
      return;
    }
    const submitted = sending.find(message => message.id === candidate.id);
    if (!submitted) {
      processingDiscsRef.current.delete(discId);
      return;
    }

    const control: QueuedMessageControl = {
      signal: pending.controller.signal,
      beginPersist: () => {
        if (pending.controller.signal.aborted) return false;
        if (!pending.preparing) return true;
        const current = loadQueue(discId);
        if (!current.some(message => message.id === submitted.id)) return false;
        if (!publishActiveQueue(discId, current.map(message => message.id === submitted.id
          ? { ...message, status: 'sending' }
          : message))) throw new Error('Unable to persist the outbox write state');
        pending.preparing = false;
        return true;
      },
    };
    let releaseAbortListener = () => {};
    const cancelled = new Promise<void>(resolve => {
      const onAbort = () => resolve();
      control.signal.addEventListener('abort', onAbort, { once: true });
      releaseAbortListener = () => control.signal.removeEventListener('abort', onAbort);
    });
    void Promise.race([
      Promise.resolve().then(() => onPersistRef.current(discId, submitted, control)),
      cancelled,
    ]).then(() => {
      if (control.signal.aborted) return;
      const current = loadQueue(discId);
      publishActiveQueue(discId, current.filter(message => message.id !== submitted.id));
    }).catch((error: unknown) => {
      if (control.signal.aborted) return;
      const current = loadQueue(discId);
      const delay = Math.min(
        RETRY_MAX_MS,
        RETRY_BASE_MS * (2 ** Math.max(0, submitted.attempts - 1)),
      );
      publishActiveQueue(discId, current.map(message => message.id === submitted.id ? {
        ...message,
        status: 'failed' as const,
        retryAt: Date.now() + delay,
        error: error instanceof Error ? error.message : String(error),
      } : message));
    }).finally(() => {
      releaseAbortListener();
      processingDiscsRef.current.delete(discId);
      setRetryTick(current => current + 1);
    });
  }, [discId, prepareBeforePersist, publishActiveQueue, queue, retryTick]);

  const enqueue = useCallback((
    content: string,
    targets?: MessageTarget[],
    targetAll?: boolean,
    replyToMessageId?: string,
  ): QueuedMessage | null => {
    const trimmed = content.trim();
    if (!trimmed || !discId) return null;
    const message: QueuedMessage = {
      id: nextId(),
      content: trimmed,
      targets,
      targetAll,
      replyToMessageId,
      status: 'queued',
      attempts: 0,
      createdAt: new Date().toISOString(),
    };
    const next = [...queueRef.current, message];
    return publishActiveQueue(discId, next) ? message : null;
  }, [discId, publishActiveQueue]);

  const removeQueued = useCallback((id: string) => {
    if (!discId) return;
    const pending = processingDiscsRef.current.get(discId);
    if (pending?.id === id && pending.preparing) pending.controller.abort();
    publishActiveQueue(
      discId,
      queueRef.current.filter(message => message.id !== id || message.status === 'sending'),
    );
  }, [discId, publishActiveQueue]);

  const retryQueued = useCallback((id: string) => {
    if (!discId) return;
    publishActiveQueue(discId, queueRef.current.map(message => message.id === id ? {
      ...message,
      status: 'queued' as const,
      retryAt: undefined,
      error: undefined,
    } : message));
  }, [discId, publishActiveQueue]);

  const clearQueue = useCallback(() => {
    if (!discId) return;
    const pending = processingDiscsRef.current.get(discId);
    if (pending?.preparing) pending.controller.abort();
    publishActiveQueue(
      discId,
      queueRef.current.filter(message => message.status === 'sending'),
    );
  }, [discId, publishActiveQueue]);

  const cancelPreparing = useCallback(() => {
    if (!discId) return;
    const pending = processingDiscsRef.current.get(discId);
    if (pending?.preparing) pending.controller.abort();
    // Pause, don't discard, unsent text. A deliberate Stop/authority change
    // requires a manual retry instead of silently publishing on a timer.
    publishActiveQueue(discId, queueRef.current.map(message => message.status === 'sending'
      ? message
      : { ...message, status: 'failed', retryAt: undefined, error: undefined }));
  }, [discId, publishActiveQueue]);

  return { queue, enqueue, removeQueued, retryQueued, clearQueue, cancelPreparing };
}
