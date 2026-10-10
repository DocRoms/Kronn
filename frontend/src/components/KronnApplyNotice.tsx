import { useState } from 'react';
import { AlertTriangle, RotateCcw } from 'lucide-react';

interface KronnApplyNoticeProps {
  /** Raw content found after the marker, shown on demand. */
  raw: string;
  onRetry: () => void;
  retryDisabled: boolean;
  t: (key: string, ...args: (string | number)[]) => string;
}

/** Shown when an agent emitted a KRONN:APPLY marker that no block could parse,
 *  so a broken proposal never fails silently. */
export function KronnApplyNotice({ raw, onRetry, retryDisabled, t }: KronnApplyNoticeProps) {
  const [showRaw, setShowRaw] = useState(false);
  return (
    <div className="wf-apicall-ai-unreadable" role="alert">
      <div className="wf-apicall-ai-unreadable-header">
        <AlertTriangle size={11} />
        <span>{t('aiHelper.apply.unreadable')}</span>
      </div>
      <div className="wf-apicall-ai-unreadable-actions">
        <button type="button" className="wf-apicall-ai-unreadable-btn" onClick={onRetry} disabled={retryDisabled}>
          <RotateCcw size={10} /> {t('aiHelper.apply.retry')}
        </button>
        <button
          type="button"
          className="wf-apicall-ai-unreadable-btn"
          onClick={() => setShowRaw(v => !v)}
          aria-expanded={showRaw}
        >
          {showRaw ? t('aiHelper.apply.hideRaw') : t('aiHelper.apply.showRaw')}
        </button>
      </div>
      {showRaw && <pre className="wf-apicall-ai-unreadable-raw">{raw}</pre>}
    </div>
  );
}
