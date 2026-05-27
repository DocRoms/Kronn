import { useState, useRef, memo } from 'react';
import { ShieldCheck, Zap, Rocket, GitBranch, Loader2, Users, Users2, Square, Star, Download, AlertTriangle } from 'lucide-react';
import type { Discussion } from '../types/generated';
import { isValidationDisc, isBriefingDisc, isBootstrapDisc } from '../lib/constants';
import { formatRelativeTime } from '../lib/relativeTime';
import { useT } from '../lib/I18nContext';
import { MatrixText } from './MatrixText';
import '../pages/DiscussionsPage.css';

const SWIPE_THRESHOLD = 80;

/** The "messages à lire" basis. Falls back to filtering the `messages` array
 *  (excluding System rows) only when the backend hasn't populated
 *  `non_system_message_count` yet — legacy clients hitting a fresh backend
 *  always get the field; the fallback covers the inverse during a partial
 *  rollout. `message_count` is the LAST resort. */
export function unseenBasis(disc: Pick<Discussion, 'message_count' | 'messages' | 'non_system_message_count'>): number {
  if (typeof disc.non_system_message_count === 'number') return disc.non_system_message_count;
  if (disc.messages?.length) return disc.messages.filter(m => m.role !== 'System').length;
  return disc.message_count ?? 0;
}

export interface SwipeableDiscItemProps {
  disc: Discussion;
  isActive: boolean;
  lastSeenCount: number;
  isSending: boolean;
  onSelect: (discId: string, msgCount: number) => void;
  onArchive: (discId: string) => void;
  onDelete: (discId: string) => void;
  /** Abort the running agent on this disc. Only rendered when `isSending`. */
  onStop?: (discId: string) => void;
  /** Toggle pin/favorite on this discussion. */
  onTogglePin?: (discId: string, pinned: boolean) => void;
  t: (key: string, ...args: (string | number)[]) => string;
  archiveLabel?: string;
  /**
   * 0.8.4 (#294) — cross-agent source binding. When set, the row
   * renders a "📥 ClaudeCode" badge so the user can see at a glance
   * that this disc was imported from an external CLI session.
   * `diverged` flips the icon to a warning when the disc has been
   * edited inside Kronn AFTER the import (a re-push would overwrite
   * the user's edits).
   */
  sourceAgent?: string | null;
  sourceDiverged?: boolean;
}

export const SwipeableDiscItem = memo(function SwipeableDiscItem({
  disc, isActive, lastSeenCount, isSending, onSelect, onArchive, onDelete, onStop, t, archiveLabel,
  sourceAgent, sourceDiverged,
}: SwipeableDiscItemProps) {
  const [offsetX, setOffsetX] = useState(0);
  const [swiping, setSwiping] = useState(false);
  const startX = useRef(0);
  const currentX = useRef(0);
  // Read the active UI locale so the relative time is rendered in the
  // right language. memo() guards against needless re-renders when the
  // locale is unchanged.
  const { locale } = useT();
  const relativeWhen = formatRelativeTime(disc.updated_at, locale);

  const handlePointerDown = (e: React.PointerEvent) => {
    startX.current = e.clientX;
    currentX.current = e.clientX;
    setSwiping(true);
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  };

  const handlePointerMove = (e: React.PointerEvent) => {
    if (!swiping) return;
    currentX.current = e.clientX;
    const delta = currentX.current - startX.current;
    const clamped = Math.sign(delta) * Math.min(Math.abs(delta) * 0.7, 120);
    setOffsetX(clamped);
  };

  const handlePointerUp = () => {
    if (!swiping) return;
    setSwiping(false);
    if (offsetX > SWIPE_THRESHOLD) {
      onArchive(disc.id);
    } else if (offsetX < -SWIPE_THRESHOLD) {
      onDelete(disc.id);
    } else if (Math.abs(offsetX) < 5) {
      onSelect(disc.id, unseenBasis(disc));
    }
    setOffsetX(0);
  };

  // The unread badge tracks USER + AGENT messages only — the streaming layer
  // persists tool calls + cached-summary lines as MessageRole::System rows,
  // which inflate `message_count`. Using `non_system_message_count` keeps
  // "50 outils dans un message" from showing up as "50 à lire".
  const unseen = unseenBasis(disc) - lastSeenCount;
  const showBadge = unseen > 0 && !isActive;
  const bgColor = offsetX > 30 ? `rgba(59,130,246,${Math.min(Math.abs(offsetX) / 120, 0.4)})`
                 : offsetX < -30 ? `rgba(239,68,68,${Math.min(Math.abs(offsetX) / 120, 0.4)})`
                 : 'transparent';
  const label = offsetX > 30 ? (archiveLabel ?? t('disc.archive')) : offsetX < -30 ? t('disc.delete') : '';

  return (
    <div className="disc-swipe-wrap">
      <div
        className="disc-swipe-bg"
        style={{
          justifyContent: offsetX > 0 ? 'flex-start' : 'flex-end',
          background: bgColor, transition: swiping ? 'none' : 'background 0.2s',
        }}
      >
        {label && <span className="disc-swipe-label">{label}</span>}
      </div>
      <div
        className="disc-item"
        data-active={isActive}
        // Accessibility: this row is interactive (click/Enter selects the
        // discussion, swipe archives/deletes). Without role+tabIndex it's
        // unreachable by keyboard. Pre-fix the JSX was a plain div with
        // pointer handlers only — Alicia's audit (2026-05-09) flagged
        // "no keyboard nav, no focus ring".
        role="button"
        tabIndex={0}
        aria-current={isActive ? 'true' : undefined}
        aria-label={`${disc.title} — ${disc.message_count ?? disc.messages.length} messages, ${disc.agent}`}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onSelect(disc.id, disc.message_count ?? disc.messages.length);
          }
        }}
        style={{
          transform: `translateX(${offsetX}px)`,
          transition: swiping ? 'none' : 'transform 0.25s ease-out',
        }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={() => { setSwiping(false); setOffsetX(0); }}
      >
        <div className="disc-item-content">
          <div className="disc-item-title">
            {isValidationDisc(disc.title) && <ShieldCheck size={10} style={{ color: 'var(--kr-accent-ink)', flexShrink: 0 }} />}
            {isBriefingDisc(disc.title) && <Zap size={10} style={{ color: 'var(--kr-info)', flexShrink: 0 }} />}
            {isBootstrapDisc(disc.title) && <Rocket size={10} style={{ color: 'var(--kr-accent-ink)', flexShrink: 0 }} />}
            {disc.workspace_mode === 'Isolated' && <GitBranch size={10} style={{ color: 'var(--kr-info)', flexShrink: 0 }} />}
            {disc.shared_id && <Users2 size={10} style={{ color: 'var(--kr-success)', flexShrink: 0 }} />}
            {/* Title text truncates; badge + star sit OUTSIDE the truncated
                zone so they're always visible even on long titles.
                0.8.5 — `title` attr exposes the disc id on hover so an
                agent referring to `04a9c927` is one mouse-over away
                (the full UUID is visible in the tooltip + searchable
                via prefix in the sidebar filter). */}
            <span
              className="disc-item-title-text"
              title={t('disc.titleHoverTooltip', disc.title, disc.id)}
            ><MatrixText text={disc.title} /></span>
            {showBadge && <span className="disc-unseen-badge">{unseen}</span>}
            {/* Pinned indicator (non-interactive) — just a small star to
                show which discs are in Favorites. The toggle lives in
                ChatHeader where there's room for a proper button. */}
            {disc.pinned && <Star size={9} style={{ color: 'var(--kr-warning)', fill: 'var(--kr-warning)', flexShrink: 0 }} />}
            {sourceAgent && (
              <span
                data-testid="disc-source-badge"
                className="disc-source-badge"
                title={sourceDiverged
                  ? t('disc.source.divergedHint', sourceAgent)
                  : t('disc.source.importedHint', sourceAgent)}
                style={{
                  display: 'inline-flex', alignItems: 'center', gap: 2,
                  fontSize: 9, padding: '1px 4px', borderRadius: 4,
                  background: sourceDiverged
                    ? 'rgba(220, 53, 69, 0.15)'
                    : 'var(--kr-bg-elevated, rgba(255,255,255,0.06))',
                  color: sourceDiverged ? 'var(--kr-danger)' : 'var(--kr-text-secondary)',
                  flexShrink: 0,
                }}
              >
                {sourceDiverged ? <AlertTriangle size={8} /> : <Download size={8} />}
                {sourceAgent}
              </span>
            )}
          </div>
          <div className="disc-item-meta">
            {isSending && <Loader2 size={8} style={{ animation: 'spin 1s linear infinite', color: 'var(--kr-accent-ink)' }} />}
            {isSending && onStop && (
              <button
                type="button"
                className="disc-item-stop-btn"
                onClick={(e) => {
                  e.stopPropagation();
                  onStop(disc.id);
                }}
                title={t('disc.stopAgent')}
                aria-label={t('disc.stopAgent')}
              >
                <Square size={8} style={{ fill: 'currentColor' }} />
              </button>
            )}
            {(disc.participants?.length ?? 0) > 1 && (
              <Users size={8} style={{ color: 'var(--kr-purple)' }} />
            )}
            {disc.message_count ?? disc.messages.length} msg · {disc.agent}
            {relativeWhen && (
              <>
                {' · '}
                {/* Dates relatives — évite de confondre plusieurs
                    discussions avec le même titre (quick prompts répétés). */}
                <span className="disc-item-relative-time" title={new Date(disc.updated_at).toLocaleString(locale)}>
                  {relativeWhen}
                </span>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
});
