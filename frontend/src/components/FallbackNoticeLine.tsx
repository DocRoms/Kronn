import { ArrowRight, Shuffle } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import type { FallbackNotice } from '../lib/fallbackNotice';

/** « Codex (gpt-5.6-sol) → Claude Code (sonnet) · quota exhausted · until 14:00 UTC »:
 *  the turn or the step moved on the fallback chain — from which agent/model,
 *  why, until when, and to which agent/model. */
export function FallbackNoticeLine({ notice }: { notice: FallbackNotice }) {
  const { t } = useT();
  return (
    <div className="fb-notice" role="note" data-testid="fallback-notice">
      <Shuffle size={11} aria-hidden="true" />
      <span className="fb-notice-from">{notice.fromAgent} <small>({notice.fromModel})</small></span>
      <ArrowRight size={11} aria-hidden="true" />
      <span className="fb-notice-to">{notice.toAgent} <small>({notice.toModel})</small></span>
      <span className="fb-notice-why">
        · {notice.reason}
        {notice.until ? ` · ${t('fallback.noticeUntil', notice.until)}` : ''}
      </span>
    </div>
  );
}
