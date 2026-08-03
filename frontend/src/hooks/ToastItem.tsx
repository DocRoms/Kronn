import { useState } from 'react';
import { Check, Copy, X } from 'lucide-react';
import type { Toast } from './toastTypes';

export function ToastItem({ toast, onDismiss }: { toast: Toast; onDismiss: () => void }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = () => {
    const payload = toast.copyable ?? toast.message;
    navigator.clipboard.writeText(payload).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1000);
    }).catch(() => {
      // Clipboard API can fail on insecure contexts or sandboxed iframes.
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
        padding: '10px 12px', borderRadius: 8, fontSize: 13,
        color: 'var(--kr-text-on-dark)',
        background: `rgba(var(--kr-${colorVar}-rgb), 0.95)`,
        border: `1px solid rgba(var(--kr-${colorVar}-rgb), 0.3)`,
        backdropFilter: 'blur(10px)', maxWidth: 420, minWidth: 240,
        animation: 'toastSlideIn 0.3s ease-out', pointerEvents: 'auto',
        display: 'flex', flexDirection: 'column', gap: 6,
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
              display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
              padding: 4, background: 'rgba(255,255,255,0.15)', border: 'none',
              borderRadius: 4, color: 'inherit', cursor: 'pointer', flexShrink: 0,
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
              display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
              padding: 4, background: 'transparent', border: 'none', color: 'inherit',
              opacity: 0.8, cursor: 'pointer', flexShrink: 0,
            }}
          >
            <X size={12} />
          </button>
        )}
      </div>
      {toast.copyable && (
        <pre style={{
          margin: 0, padding: '6px 8px', fontSize: 11, lineHeight: 1.4,
          background: 'rgba(0,0,0,0.25)', borderRadius: 4, maxHeight: 240,
          overflow: 'auto', whiteSpace: 'pre-wrap', wordBreak: 'break-word',
          userSelect: 'text',
          fontFamily: 'var(--kr-font-mono, ui-monospace, SFMono-Regular, Menlo, monospace)',
        }}>
          {toast.copyable}
        </pre>
      )}
    </div>
  );
}
