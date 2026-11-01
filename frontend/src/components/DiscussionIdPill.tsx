// KT-1530 — the discussion's copyable id, next to the title in the header.
//
// 0.8.5 — short disc-id pill. Surfaces the id so a user reading an agent
// summary like "Disc 3 — 04a9c927" can click → copy the full UUID and paste it
// anywhere (next disc, linked-issue field, Slack message…). It lives on the
// title row, never behind "Details": people click it all the time to mention
// the discussion to an agent. Sidebar search also matches id prefix.
import { useEffect, useRef, useState } from 'react';
import { Check } from 'lucide-react';
import type { ToastFn } from '../hooks/useToast';

export interface DiscussionIdPillProps {
  discussionId: string;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function DiscussionIdPill({ discussionId, toast, t }: DiscussionIdPillProps) {
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (resetTimer.current) clearTimeout(resetTimer.current);
  }, []);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(discussionId);
      setCopied(true);
      if (resetTimer.current) clearTimeout(resetTimer.current);
      resetTimer.current = setTimeout(() => setCopied(false), 1500);
      toast(t('disc.idCopied'), 'success');
    } catch {
      setCopied(false);
      toast(t('disc.idCopyFailed'), 'error');
    }
  };

  return (
    <button
      type="button"
      className="disc-id-pill"
      data-tour-id="discussion-id-pill"
      data-copied={copied}
      onClick={(e) => {
        e.stopPropagation();
        void copy();
      }}
      title={t('disc.idPillTooltip', discussionId)}
      aria-label={t('disc.idPillTooltip', discussionId)}
    >
      {copied ? <Check size={8} /> : null}
      #{discussionId.slice(0, 8)}
    </button>
  );
}
