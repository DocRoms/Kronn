import { useState, useCallback, useRef } from 'react';
import { X, Copy, Check } from 'lucide-react';

type ToastType = 'success' | 'error' | 'warning' | 'info';

interface ToastOptions {
  /** Stay visible until manually dismissed. Defaults to `true` for `error`,
   *  `false` otherwise. Errors interrupt flow and need copy/diagnostic time;
   *  success/info confirm an action the user already initiated. */
  persistent?: boolean;
  /** Optional long-form content (usually stderr / stack) rendered in a
   *  monospace <pre> with a copy button. Text is selectable and scrollable.
   *  Pass `undefined` when the `message` is self-sufficient. */
  copyable?: string;
  /** Override the dedup behaviour. By default identical (message, type)
   *  toasts fired within `DEDUP_WINDOW_MS` collapse into the existing
   *  one — protects against multi-agent / batch flows that broadcast
   *  the same WS event N times in rapid succession (the user reported
   *  N "discussion finished" toasts during a 4-agent compare-agents
   *  run on 2026-05-10). Set `dedup: false` to opt out, e.g. when the
   *  user genuinely just hit the same button twice on purpose. */
  dedup?: boolean;
}

interface Toast {
  id: number;
  message: string;
  type: ToastType;
  persistent: boolean;
  copyable: string | null;
}

export type ToastFn = (message: string, type?: ToastType, options?: ToastOptions) => void;

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

interface ToastItemProps {
  toast: Toast;
  onDismiss: () => void;
}

function ToastItem({ toast, onDismiss }: ToastItemProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = () => {
    const payload = toast.copyable ?? toast.message;
    navigator.clipboard.writeText(payload).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1000);
    }).catch(() => {
      // Clipboard API can fail on insecure contexts or sandboxed iframes.
      // We don't surface a sub-toast — keeping the error quiet matches how
      // most dev-tools handle this (the button just doesn't swap icons).
    });
  };

  const colorVar = toast.type === 'error' ? 'error'
    : toast.type === 'success' ? 'success'
    : toast.type === 'warning' ? 'warning'
    : 'cyan';

  return (
    <div
      role="alert"
      aria-live={toast.type === 'error' || toast.type === 'warning' ? 'assertive' : 'polite'}
      className="kr-toast"
      data-type={toast.type}
      style={{
        padding: '10px 12px',
        borderRadius: 8,
        fontSize: 13,
        color: 'var(--kr-text-on-dark)',
        background: `rgba(var(--kr-${colorVar}-rgb), 0.95)`,
        border: `1px solid rgba(var(--kr-${colorVar}-rgb), 0.3)`,
        backdropFilter: 'blur(10px)',
        maxWidth: 420,
        minWidth: 240,
        animation: 'toastSlideIn 0.3s ease-out',
        pointerEvents: 'auto',
        display: 'flex',
        flexDirection: 'column',
        gap: 6,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'flex-start', gap: 8 }}>
        <div style={{ flex: 1, wordBreak: 'break-word', userSelect: 'text' }}>
          {toast.message}
        </div>
        {toast.copyable && (
          <button
            type="button"
            onClick={handleCopy}
            aria-label="Copy"
            title="Copy"
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              padding: 4,
              background: 'rgba(255,255,255,0.15)',
              border: 'none',
              borderRadius: 4,
              color: 'inherit',
              cursor: 'pointer',
              flexShrink: 0,
            }}
          >
            {copied ? <Check size={12} /> : <Copy size={12} />}
          </button>
        )}
        {toast.persistent && (
          <button
            type="button"
            onClick={onDismiss}
            aria-label="Close"
            title="Close"
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              padding: 4,
              background: 'transparent',
              border: 'none',
              color: 'inherit',
              opacity: 0.8,
              cursor: 'pointer',
              flexShrink: 0,
            }}
          >
            <X size={12} />
          </button>
        )}
      </div>

      {toast.copyable && (
        <pre
          style={{
            margin: 0,
            padding: '6px 8px',
            fontSize: 11,
            lineHeight: 1.4,
            background: 'rgba(0,0,0,0.25)',
            borderRadius: 4,
            maxHeight: 240,
            overflow: 'auto',
            whiteSpace: 'pre-wrap',
            wordBreak: 'break-word',
            userSelect: 'text',
            fontFamily: 'var(--kr-font-mono, ui-monospace, SFMono-Regular, Menlo, monospace)',
          }}
        >
          {toast.copyable}
        </pre>
      )}
    </div>
  );
}

/** Coalesce window for identical (message, type) toasts. 1.5s is wider
 *  than the typical WS broadcast burst from a multi-agent flow but
 *  short enough that an intentional re-fire after a second action
 *  shows up. */
const DEDUP_WINDOW_MS = 1500;

export function useToast() {
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
  }, [dismiss]);

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
            zIndex: 9999,
            display: 'flex',
            flexDirection: 'column',
            gap: 8,
            pointerEvents: 'none',
          }}
        >
          {toasts.map(t => (
            <ToastItem key={t.id} toast={t} onDismiss={() => dismiss(t.id)} />
          ))}
        </div>
      </>
    );
  }, [toasts, dismiss]);

  return { toast, ToastContainer };
}
