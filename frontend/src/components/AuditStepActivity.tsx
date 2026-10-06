/**
 * The running audit step's latest actions behind a "Details" toggle, so a
 * 10-minute step reads as work in progress rather than a stuck one. Entries
 * arrive sanitized and bounded from the server.
 */
import { useEffect, useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import type { AuditRecentActivity } from '../types/generated';

const OPEN_KEY_PREFIX = 'kr.audit.activity.open.';

function readOpen(projectId: string): boolean {
  try { return sessionStorage.getItem(OPEN_KEY_PREFIX + projectId) === '1'; } catch { return false; }
}

function writeOpen(projectId: string, open: boolean) {
  try {
    if (open) sessionStorage.setItem(OPEN_KEY_PREFIX + projectId, '1');
    else sessionStorage.removeItem(OPEN_KEY_PREFIX + projectId);
  } catch { /* storage unavailable: the toggle still works for this view */ }
}

function relativeTime(iso: string, now: number, locale: string): string {
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return '';
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  const format = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'narrow' });
  if (seconds < 60) return format.format(-seconds, 'second');
  if (seconds < 3600) return format.format(-Math.floor(seconds / 60), 'minute');
  return format.format(-Math.floor(seconds / 3600), 'hour');
}

export function AuditStepActivity({ projectId, recent }: { projectId: string; recent: AuditRecentActivity | null }) {
  const { t, locale } = useT();
  const [open, setOpen] = useState(() => readOpen(projectId));
  const listId = `audit-step-activity-${projectId}`;
  const [now, setNow] = useState(() => Date.now());
  const toggle = () => {
    setNow(Date.now());
    setOpen(prev => {
      writeOpen(projectId, !prev);
      return !prev;
    });
  };
  const entries = recent?.entries ?? [];
  // Ages tick while the list is open.
  useEffect(() => {
    if (!open) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [open]);
  return (
    <div className="audit-tl-activity" data-testid="audit-step-activity">
      <button
        type="button"
        className="audit-tl-activity-toggle"
        aria-expanded={open}
        aria-controls={listId}
        onClick={toggle}
      >
        {t('auditTimeline.activity.toggle')}
        {open ? <ChevronDown size={12} aria-hidden="true" /> : <ChevronRight size={12} aria-hidden="true" />}
      </button>
      {open && (
        <div id={listId} className="audit-tl-activity-body">
          {entries.length === 0 ? (
            <p className="audit-tl-muted" data-testid="audit-step-activity-empty">{t('auditTimeline.activity.empty')}</p>
          ) : (
            <ul className="audit-tl-activity-list" aria-label={t('auditTimeline.activity.label')}>
              {entries.map((entry, i) => (
                <li key={`${entry.at}-${i}`} data-testid="audit-step-activity-entry">
                  <span className="audit-tl-activity-action" title={entry.target ? `${entry.tool} ${entry.target}` : entry.tool}>
                    <span className="audit-tl-activity-tool">{entry.tool}</span>
                    {entry.target && <span className="audit-tl-activity-target"> {entry.target}</span>}
                  </span>
                  <time className="audit-tl-activity-time" dateTime={entry.at}>{relativeTime(entry.at, now, locale)}</time>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
