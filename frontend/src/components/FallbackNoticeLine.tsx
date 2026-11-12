import { ArrowRight, Shuffle } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import type { FallbackNotice } from '../lib/fallbackNotice';
import { TransportBadge } from './settings/FallbackRow';
import './settings/AgentFallbackControl.css';

/** « Codex · gpt-5.6-sol → Claude Code · sonnet [CLI] · quota exhausted · until 14:00 UTC »:
 *  the turn or the step moved on to a fallback — from which agent/model, why,
 *  until when, and to which agent/model over which transport. */
export function FallbackNoticeLine({ notice }: { notice: FallbackNotice }) {
  const { t } = useT();
  return (
    <div className="fb-notice" role="note" data-testid="fallback-notice">
      <Shuffle size={11} aria-hidden="true" />
      <span className="fb-notice-from">{notice.fromAgent} <small>· {notice.fromModel}</small></span>
      <ArrowRight size={11} aria-hidden="true" />
      <span className="fb-notice-to">{notice.toAgent} <small>· {notice.toModel}</small></span>
      {notice.toTransport && <TransportBadge transport={notice.toTransport} />}
      <span className="fb-notice-why">
        · {notice.reason}
        {notice.until ? ` · ${t('fallback.noticeUntil', notice.until)}` : ''}
      </span>
    </div>
  );
}
