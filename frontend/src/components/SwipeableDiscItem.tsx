import { useState, useRef, useEffect, memo } from 'react';
import {
  ShieldCheck, Zap, Rocket, GitBranch, Loader2, Users, Users2, Square, Star,
  Link2, Download, AlertTriangle, Check, MoreHorizontal, Copy, Archive, Trash2,
} from 'lucide-react';
import type { Discussion } from '../types/generated';
import { isValidationDisc, isBriefingDisc, isBootstrapDisc } from '../lib/constants';
import { formatRelativeTime } from '../lib/relativeTime';
import { sourceAgentShortLabel, unseenBasis } from '../lib/discussionUiUtils';
import { gravatarUrl } from '../lib/gravatar';
import { useT } from '../lib/I18nContext';
import { MatrixText } from './MatrixText';
import '../pages/DiscussionsPage.css';

const SWIPE_THRESHOLD = 80;

export interface SwipeableDiscItemProps {
  disc: Discussion;
  isActive: boolean;
  lastSeenCount: number;
  isSending: boolean;
  /** The disc's agent is CREATED but throttled (waiting its turn in a
   *  batch). Shows a dimmed "en file" dot instead of the active spinner, so
   *  a big batch doesn't render N identical running spinners. Ignored when
   *  `isSending` (running wins over queued). */
  isQueued?: boolean;
  onSelect: (discId: string, msgCount: number) => void;
  onArchive: (discId: string) => void;
  onDelete: (discId: string) => void;
  selectionMode?: boolean;
  isSelected?: boolean;
  onToggleSelection?: (discId: string) => void;
  /** Abort the running agent on this disc. Only rendered when `isSending`. */
  onStop?: (discId: string) => void;
  /** Toggle pin/favorite on this discussion. */
  onTogglePin?: (discId: string, pinned: boolean) => void;
  /** Project / workspace label used by cross-project shortcut sections. */
  contextLabel?: string;
  t: (key: string, ...args: (string | number)[]) => string;
  archiveLabel?: string;
  /**
   * 0.8.4 (#294) — cross-agent source binding: this disc is BOUND to a live
   * external CLI session, which is not the same thing as having been imported
   * from one. The badge says so ("Liée à ClaudeCode"); a real portable-bundle
   * import is a separate provenance (KT-74).
   * `diverged` flips the icon to a warning when the disc has been edited
   * inside Kronn after the last sync (a re-push would overwrite those edits).
   */
  /** KT-85 — one entry per CLI session bound to this disc. A cross-agent room
   *  has several; a single value silently showed only one of them. */
  sourceAgents?: { source_agent: string; diverged: boolean }[] | null;
  /**
   * KT-74 — this disc came from a portable bundle. `pseudo`/`avatarEmail` are
   * null when the exporting instance had no identity configured: a real import
   * by an unknown person, rendered without an author instead of a placeholder.
   */
  importedBy?: { pseudo: string | null; avatarEmail: string | null } | null;
}

export const SwipeableDiscItem = memo(function SwipeableDiscItem({
  disc, isActive, lastSeenCount, isSending, isQueued = false, onSelect, onArchive, onDelete, onStop, t, archiveLabel,
  sourceAgents, importedBy, selectionMode = false, isSelected = false, onToggleSelection, onTogglePin,
  contextLabel,
}: SwipeableDiscItemProps) {
  const [offsetX, setOffsetX] = useState(0);
  const [swiping, setSwiping] = useState(false);
  const startX = useRef(0);
  const currentX = useRef(0);
  const suppressNextClickRef = useRef(false);
  const actionMenuRef = useRef<HTMLDivElement>(null);
  const actionMenuButtonRef = useRef<HTMLButtonElement>(null);
  const [actionMenuOpen, setActionMenuOpen] = useState(false);
  const [actionMenuPlacement, setActionMenuPlacement] = useState<'up' | 'down'>('down');
  const [copied, setCopied] = useState(false);
  // Read the active UI locale so the relative time is rendered in the
  // right language. memo() guards against needless re-renders when the
  // locale is unchanged.
  const { locale } = useT();
  const relativeWhen = formatRelativeTime(disc.updated_at, locale);

  const handlePointerDown = (e: React.PointerEvent) => {
    if (selectionMode) return;
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
    if (selectionMode) return;
    if (!swiping) return;
    setSwiping(false);
    // Pointer gestures are followed by a synthetic click in browsers. The
    // pointer branch owns this interaction; suppress that click so selection,
    // archive or delete never fires twice.
    suppressNextClickRef.current = true;
    if (offsetX > SWIPE_THRESHOLD) {
      onArchive(disc.id);
    } else if (offsetX < -SWIPE_THRESHOLD) {
      onDelete(disc.id);
    } else if (Math.abs(offsetX) < 5) {
      onSelect(disc.id, unseenBasis(disc));
    }
    setOffsetX(0);
  };

  useEffect(() => {
    if (!actionMenuOpen) return;
    const closeFromOutside = (event: PointerEvent) => {
      if (!actionMenuRef.current?.contains(event.target as Node)) setActionMenuOpen(false);
    };
    const closeFromKeyboard = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setActionMenuOpen(false);
    };
    window.addEventListener('pointerdown', closeFromOutside);
    window.addEventListener('keydown', closeFromKeyboard);
    return () => {
      window.removeEventListener('pointerdown', closeFromOutside);
      window.removeEventListener('keydown', closeFromKeyboard);
    };
  }, [actionMenuOpen]);

  const handleOpenClick = () => {
    if (suppressNextClickRef.current) {
      suppressNextClickRef.current = false;
      return;
    }
    if (selectionMode) onToggleSelection?.(disc.id);
    else onSelect(disc.id, msgCount);
  };

  const copyDiscussionId = async () => {
    try {
      await navigator.clipboard.writeText(disc.id);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard permission failures leave the menu open so the user can
      // retry; never show a false success state.
    }
  };

  // The unread badge AND the visible "N msg" label track USER + AGENT messages
  // only — the streaming layer persists tool calls + cached-summary lines + the
  // enforce refusal note as MessageRole::System rows, which inflate
  // `message_count`. `unseenBasis` resolves to `non_system_message_count`, so
  // "50 outils dans un message" never shows up as "50 messages" / "50 à lire".
  const msgCount = unseenBasis(disc);
  const unseen = msgCount - lastSeenCount;
  const showBadge = unseen > 0 && !isActive;
  const bgColor = offsetX > 30 ? `rgba(59,130,246,${Math.min(Math.abs(offsetX) / 120, 0.4)})`
                 : offsetX < -30 ? `rgba(239,68,68,${Math.min(Math.abs(offsetX) / 120, 0.4)})`
                 : 'transparent';
  const label = offsetX > 30 ? (archiveLabel ?? t('disc.archive')) : offsetX < -30 ? t('disc.delete') : '';

  return (
    <div className="disc-swipe-wrap" data-menu-open={actionMenuOpen}>
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
        data-tour-disc-id={disc.id}
        data-active={isActive}
        data-selected={selectionMode && isSelected}
        style={{
          transform: `translateX(${offsetX}px)`,
          transition: swiping ? 'none' : 'transform 0.25s ease-out',
        }}
      >
        <button
          type="button"
          className="disc-item-open"
          role={selectionMode ? 'checkbox' : undefined}
          aria-current={!selectionMode && isActive ? 'true' : undefined}
          aria-checked={selectionMode ? isSelected : undefined}
          aria-label={`${disc.title} — ${msgCount} messages, ${disc.agent}`}
          onClick={handleOpenClick}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={() => { setSwiping(false); setOffsetX(0); }}
        >
          {selectionMode && (
            <span className="disc-item-selection-box" data-selected={isSelected} aria-hidden="true">
              {isSelected && <Check size={11} />}
            </span>
          )}
          {!selectionMode && (
            <span
              className="disc-item-status-dot"
              data-state={isSending ? 'running' : isQueued ? 'queued' : showBadge ? 'unread' : 'idle'}
              aria-hidden="true"
            />
          )}
          <span className="disc-item-content">
          <div className="disc-item-title">
            {isValidationDisc(disc.title) && <ShieldCheck size={10} style={{ color: 'var(--kr-accent-ink)', flexShrink: 0 }} />}
            {isBriefingDisc(disc.title) && <Zap size={10} style={{ color: 'var(--kr-info)', flexShrink: 0 }} />}
            {isBootstrapDisc(disc.title) && <Rocket size={10} style={{ color: 'var(--kr-accent-ink)', flexShrink: 0 }} />}
            {(disc.workspace_mode === 'Isolated'
              || disc.shared_id
              || (disc.participants?.length ?? 0) > 1) && (
              <span className="disc-item-state-cluster">
                {disc.workspace_mode === 'Isolated' && (
                  <span
                    className="disc-item-state-icon"
                    title={t('disc.workspaceIsolated')}
                    aria-label={t('disc.workspaceIsolated')}
                    data-kind="workspace"
                  >
                    <GitBranch size={10} aria-hidden="true" />
                  </span>
                )}
                {disc.shared_id ? (
                  <span
                    className="disc-item-state-icon"
                    title={t('disc.sidebar.sharedDiscussion')}
                    aria-label={t('disc.sidebar.sharedDiscussion')}
                    data-kind="shared"
                  >
                    <Users2 size={10} aria-hidden="true" />
                  </span>
                ) : (disc.participants?.length ?? 0) > 1 ? (
                  <span
                    className="disc-item-state-icon"
                    title={t('disc.sidebar.multiAgentDiscussion')}
                    aria-label={t('disc.sidebar.multiAgentDiscussion')}
                    data-kind="multi-agent"
                  >
                    <Users size={10} aria-hidden="true" />
                  </span>
                ) : null}
              </span>
            )}
            {/* The state cluster stays before the flexible title so it is
                discoverable without taking the title's trailing width.
                0.8.5 — `title` attr exposes the disc id on hover so an
                agent referring to `04a9c927` is one mouse-over away
                (the full UUID is visible in the tooltip + searchable
                via prefix in the sidebar filter). */}
            <span
              className="disc-item-title-text"
              title={t('disc.titleHoverTooltip', disc.title, disc.id)}
            ><MatrixText text={disc.title} /></span>
            {showBadge && <span className="disc-unseen-badge">{unseen}</span>}
          </div>
          <div className="disc-item-meta">
            {/* Queued (throttled, not yet running): a static hourglass,
                NOT the active spinner. Running always wins over queued. */}
            {!isSending && isQueued && (
              <span
                className="disc-item-queued"
                role="img"
                title={t('disc.queued')}
                aria-label={t('disc.queued')}
              >
                ⏳
              </span>
            )}
            {isSending && <Loader2 size={8} style={{ animation: 'spin 1s linear infinite', color: 'var(--kr-accent-ink)' }} />}
            <span className="disc-item-meta-summary">
              {contextLabel && (
                <>
                  {contextLabel}
                  {' · '}
                </>
              )}
              {msgCount} msg · {disc.agent}
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
            </span>
            {((sourceAgents?.length ?? 0) > 0 || importedBy) && (
              <span className="disc-source-badges" aria-label={t('disc.source.filterTooltip')}>
                {(sourceAgents ?? []).map(({ source_agent, diverged }) => {
                  const accessibleLabel = t('disc.source.boundBadge', source_agent);
                  return (
                    <span
                      key={source_agent}
                      data-testid="disc-source-badge"
                      className="disc-source-badge disc-source-badge--compact"
                      title={diverged
                        ? t('disc.source.divergedHint', source_agent)
                        : t('disc.source.boundHint', source_agent)}
                      aria-label={accessibleLabel}
                      data-diverged={diverged}
                    >
                      {diverged ? <AlertTriangle size={8} /> : <Link2 size={8} />}
                      <span aria-hidden="true">{sourceAgentShortLabel(source_agent)}</span>
                    </span>
                  );
                })}
                {importedBy && (
                  <span
                    data-testid="disc-import-badge"
                    className="disc-source-badge disc-source-badge--compact"
                    title={importedBy.pseudo
                      ? t('disc.import.byHint', importedBy.pseudo)
                      : t('disc.import.anonymousHint')}
                    aria-label={importedBy.pseudo
                      ? t('disc.import.byBadge', importedBy.pseudo)
                      : t('disc.import.anonymousBadge')}
                  >
                    {importedBy.avatarEmail
                      ? <img
                          src={gravatarUrl(importedBy.avatarEmail, 16)}
                          alt=""
                          width={10}
                          height={10}
                        />
                      : <Download size={8} />}
                    <span aria-hidden="true">IM</span>
                  </span>
                )}
              </span>
            )}
          </div>
          </span>
        </button>
        {!selectionMode && (
          <div className="disc-item-actions" ref={actionMenuRef}>
            {isSending && onStop && (
              <button
                type="button"
                className="disc-item-stop-btn"
                onClick={() => onStop(disc.id)}
                title={t('disc.stopAgent')}
                aria-label={t('disc.stopAgent')}
              >
                <Square size={8} style={{ fill: 'currentColor' }} />
              </button>
            )}
            <button
              ref={actionMenuButtonRef}
              type="button"
              className="disc-item-more-btn"
              onClick={() => {
                if (!actionMenuOpen) {
                  const rect = actionMenuButtonRef.current?.getBoundingClientRect();
                  setActionMenuPlacement(rect && window.innerHeight - rect.bottom < 180 ? 'up' : 'down');
                }
                setActionMenuOpen(open => !open);
              }}
              aria-label={t('disc.actions')}
              aria-expanded={actionMenuOpen}
              title={t('disc.actions')}
            >
              <MoreHorizontal size={14} />
            </button>
            {actionMenuOpen && (
              <div
                className="disc-item-action-menu"
                role="menu"
                data-placement={actionMenuPlacement}
              >
                {onTogglePin && (
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      onTogglePin(disc.id, !disc.pinned);
                      setActionMenuOpen(false);
                    }}
                  >
                    <Star size={12} />
                    {t(disc.pinned ? 'disc.unpin' : 'disc.pin')}
                  </button>
                )}
                <button type="button" role="menuitem" data-copied={copied} onClick={() => void copyDiscussionId()}>
                  {copied ? <Check size={12} /> : <Copy size={12} />}
                  {t('disc.copyId')}
                </button>
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    onArchive(disc.id);
                    setActionMenuOpen(false);
                  }}
                >
                  <Archive size={12} />
                  {archiveLabel ?? t('disc.archive')}
                </button>
                <button
                  type="button"
                  role="menuitem"
                  className="disc-item-action-danger"
                  onClick={() => {
                    onDelete(disc.id);
                    setActionMenuOpen(false);
                  }}
                >
                  <Trash2 size={12} />
                  {t('disc.delete')}
                </button>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
});
