import { useState, useRef, memo } from 'react';
import { ShieldCheck, Zap, Rocket, GitBranch, Loader2, Users, Users2, Square, Star } from 'lucide-react';
import type { Discussion } from '../types/generated';
import { isValidationDisc } from '../lib/constants';
import { formatRelativeTime } from '../lib/relativeTime';
import { useT } from '../lib/I18nContext';
import { MatrixText } from './MatrixText';
import '../pages/DiscussionsPage.css';

const isBootstrapDisc = (title: string) => title.startsWith('Bootstrap: ');
const isBriefingDisc = (title: string) => title.startsWith('Briefing');

const SWIPE_THRESHOLD = 80;

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
  t: (key: string, ...args: any[]) => string;
  archiveLabel?: string;
}

export const SwipeableDiscItem = memo(function SwipeableDiscItem({
  disc, isActive, lastSeenCount, isSending, onSelect, onArchive, onDelete, onStop, t, archiveLabel,
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
      onSelect(disc.id, disc.message_count ?? disc.messages.length);
    }
    setOffsetX(0);
  };

  const unseen = (disc.message_count ?? disc.messages.length) - lastSeenCount;
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
                zone so they're always visible even on long titles. */}
            <span className="disc-item-title-text"><MatrixText text={disc.title} /></span>
            {showBadge && <span className="disc-unseen-badge">{unseen}</span>}
            {/* Pinned indicator (non-interactive) — just a small star to
                show which discs are in Favorites. The toggle lives in
                ChatHeader where there's room for a proper button. */}
            {disc.pinned && <Star size={9} style={{ color: 'var(--kr-warning)', fill: 'var(--kr-warning)', flexShrink: 0 }} />}
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
