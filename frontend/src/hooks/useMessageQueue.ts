import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { MessageTarget } from '../types/generated';

export type QueuedMessageStatus = 'queued' | 'sending' | 'failed';

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
}: {
  discId: string | null;
  onPersist: (discId: string, message: QueuedMessage) => Promise<void>;
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
  const processingDiscsRef = useRef(new Set<string>());
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
      || (message.status === 'failed' && (message.retryAt ?? 0) <= now)
    ));
    if (!candidate) return;

    processingDiscsRef.current.add(discId);
    const sending = queue.map(message => message.id === candidate.id ? {
      ...message,
      status: 'sending' as const,
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

    void onPersistRef.current(discId, submitted).then(() => {
      const current = loadQueue(discId);
      publishActiveQueue(discId, current.filter(message => message.id !== submitted.id));
    }).catch((error: unknown) => {
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
      processingDiscsRef.current.delete(discId);
      setRetryTick(current => current + 1);
    });
  }, [discId, publishActiveQueue, queue, retryTick]);

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
    publishActiveQueue(
      discId,
      queueRef.current.filter(message => message.status === 'sending'),
    );
  }, [discId, publishActiveQueue]);

  return { queue, enqueue, removeQueued, retryQueued, clearQueue };
}
