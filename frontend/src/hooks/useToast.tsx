import { useState, useCallback, useRef } from 'react';
import { useT } from '../lib/I18nContext';
import { isServerFailureMessage } from '../lib/notifications';
import { ToastItem } from './ToastItem';
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

let styleInjected = false;

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
    // separately. The cap-to-3 (`prev.slice(-2)` below) limited the
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

    setToasts(prev => [...prev.slice(-2), { id, message, type, persistent, copyable }]);

    if (!persistent) {
      window.setTimeout(() => dismiss(id), AUTO_DISMISS_MS[type]);
    }
  }, [dismiss, t]);

  const ToastContainer = useCallback(() => {
    if (!styleInjected) styleInjected = true;
    return (
      <>
        <style>{`
          @keyframes toastSlideIn {
            from { transform: translateX(100%); opacity: 0; }
            to { transform: translateX(0); opacity: 1; }
          }
        `}</style>
        <div
          style={{
            position: 'fixed',
            top: 16,
            right: 16,
            zIndex: 'var(--kr-z-toast)',
            display: 'flex',
            flexDirection: 'column',
            gap: 8,
            pointerEvents: 'none',
          }}
        >
          {toasts.map(item => (
            <ToastItem key={item.id} toast={item} onDismiss={() => dismiss(item.id)} />
          ))}
        </div>
      </>
    );
  }, [toasts, dismiss]);

  return { toast, ToastContainer };
}
