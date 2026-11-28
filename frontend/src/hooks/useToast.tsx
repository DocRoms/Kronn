import { useState, useCallback, useRef } from 'react';
import { useT } from '../lib/I18nContext';
import { isServerFailureMessage } from '../lib/notifications';
import { ToastStack } from './ToastStack';
import type { Toast, ToastFn, ToastType } from './toastTypes';

export type { ToastFn } from './toastTypes';

const AUTO_DISMISS_MS: Record<ToastType, number> = {
  success: 3000,
  info: 5000,
  // Warnings stay a bit longer than info — they signal "something is off
  // but not fatal" and the user typically wants to read the full text.
  warning: 7000,
  // Not used when persistent — see useToast below.
  error: 0,
};

/** Coalesce window for identical (message, type) toasts. 1.5s is wider
 *  than the typical WS broadcast burst from a multi-agent flow but
 *  short enough that an intentional re-fire after a second action
 *  shows up. */
const DEDUP_WINDOW_MS = 1500;

/** KT-1684 — server errors and outages live in the bell: at most one short
 *  "see in the bell" toast in this window, whoever fires it. */
const SERVER_FAILURE_TOAST_WINDOW_MS = 30_000;
let lastServerFailureToastAt = 0;

/** For tests: the "see in the bell" toast may fire again. */
export function resetServerFailureToast(): void {
  lastServerFailureToastAt = 0;
}

/** KT-1883 — at most this many transient toasts on screen; the oldest goes
 *  first. A persistent toast (an error by default) is never evicted: it stays
 *  until the user dismisses it, whatever comes after it. */
export const MAX_TRANSIENT_TOASTS = 3;

/** The stack once `item` is added: every persistent toast, and the newest
 *  MAX_TRANSIENT_TOASTS transient ones, in arrival order. */
export function withToast(prev: readonly Toast[], item: Toast): Toast[] {
  const next = [...prev, item];
  const transient = next.filter(t => !t.persistent);
  const evicted = new Set(transient.slice(0, Math.max(0, transient.length - MAX_TRANSIENT_TOASTS)).map(t => t.id));
  return next.filter(t => !evicted.has(t.id));
}

export function useToast() {
  const { t } = useT();
  const [toasts, setToasts] = useState<Toast[]>([]);
  const idRef = useRef(0);
  /** Last-seen-at map keyed by `${type}::${message}`. Persists for the
   *  hook's lifetime — cleanup on unmount happens by closure GC. We
   *  intentionally don't tie this to React state because the dedup
   *  decision must be synchronous and not depend on a re-render. */
  const lastSeenRef = useRef<Map<string, number>>(new Map());

  const dismiss = useCallback((id: number) => {
    setToasts(prev => prev.filter(t => t.id !== id));
  }, []);

  const toast: ToastFn = useCallback((message, type = 'info', options) => {
    // A 5xx or an unreachable backend is a notification, not a stack of toasts
    // (4xx stay next to the action: they are not matched here).
    if (type === 'error' && isServerFailureMessage(message)) {
      const at = Date.now();
      if (at - lastServerFailureToastAt < SERVER_FAILURE_TOAST_WINDOW_MS) return;
      lastServerFailureToastAt = at;
      message = t('notif.toast.seeBell');
      type = 'warning';
      options = { persistent: false, dedup: false };
    }
    // Dedup-by-content: if the SAME (message, type) was fired in the
    // last DEDUP_WINDOW_MS, drop the new fire. Pin: TD-20260510-multi-
    // agent-disc-finished-toasts — multi-agent QP runs broadcast N
    // identical "batch finished" events in rapid succession from
    // multiple WS subscribers in the dashboard, each calling toast()
    // separately. The cap-to-3 (`withToast` below) limited the
    // visible damage but didn't fix the root cause. This dedup does.
    const dedupEnabled = options?.dedup ?? true;
    if (dedupEnabled) {
      const key = `${type}::${message}`;
      const now = Date.now();
      const lastSeen = lastSeenRef.current.get(key);
      if (lastSeen && now - lastSeen < DEDUP_WINDOW_MS) {
        return;
      }
      lastSeenRef.current.set(key, now);
    }

    const id = ++idRef.current;
    // Errors are persistent by default — they require user attention,
    // often need to be copied, and the user explicitly validated this
    // pattern. Override with `persistent: false` if you really need an
    // ephemeral error (e.g. transient network blips).
    const persistent = options?.persistent ?? type === 'error';
    const copyable = options?.copyable ?? null;

    setToasts(prev => withToast(prev, { id, message, type, persistent, copyable }));

    if (!persistent) {
      window.setTimeout(() => dismiss(id), AUTO_DISMISS_MS[type]);
    }
  }, [dismiss, t]);

  const ToastContainer = useCallback(
    () => <ToastStack toasts={toasts} onDismiss={dismiss} />,
    [toasts, dismiss],
  );

  return { toast, ToastContainer };
}
