import { useState, useRef, memo } from 'react';
import { ShieldCheck, Zap, Rocket, GitBranch, Loader2, Users, Users2 } from 'lucide-react';
import type { Discussion } from '../types/generated';
import { isValidationDisc } from '../lib/constants';
import { formatRelativeTime } from '../lib/relativeTime';
import { useT } from '../lib/I18nContext';
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
  t: (key: string, ...args: any[]) => string;
  archiveLabel?: string;
}

export const SwipeableDiscItem = memo(function SwipeableDiscItem({
  disc, isActive, lastSeenCount, isSending, onSelect, onArchive, onDelete, t, archiveLabel,
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
            {isValidationDisc(disc.title) && <ShieldCheck size={10} style={{ color: '#c8ff00', flexShrink: 0 }} />}
            {isBriefingDisc(disc.title) && <Zap size={10} style={{ color: '#60a5fa', flexShrink: 0 }} />}
            {isBootstrapDisc(disc.title) && <Rocket size={10} style={{ color: '#c8ff00', flexShrink: 0 }} />}
            {disc.workspace_mode === 'Isolated' && <GitBranch size={10} style={{ color: '#60a5fa', flexShrink: 0 }} />}
            {disc.shared_id && <Users2 size={10} style={{ color: '#34d399', flexShrink: 0 }} />}
            {disc.title}
            {showBadge && <span className="disc-unseen-badge">{unseen}</span>}
          </div>
          <div className="disc-item-meta">
            {isSending && <Loader2 size={8} style={{ animation: 'spin 1s linear infinite', color: '#c8ff00' }} />}
            {(disc.participants?.length ?? 0) > 1 && (
              <Users size={8} style={{ color: '#8b5cf6' }} />
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
