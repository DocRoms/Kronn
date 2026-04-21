import { useState, useRef, useEffect, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { AgentProfile } from '../types/generated';

interface ProfileTooltipProps {
  profile: AgentProfile;
  children: ReactNode;
  /** Hard cap on the `persona_prompt` chars rendered. Above this the
   *  text is truncated with an ellipsis — full prompts can be 2-4 KB
   *  and no user will watch a 2-minute auto-scroll. 800 is tuned to
   *  give ~35-40 seconds at the reading pace set below, which is the
   *  upper bound for a hover interaction. */
  maxPromptChars?: number;
  /** Delay before the tooltip appears, in ms. Prevents flicker when the
   *  user is mousing through a grid of chips. */
  openDelayMs?: number;
}

/** Pixels per second for the content auto-scroll once the tooltip is
 *  open and the persona_prompt overflows the fixed `maxHeight`. Tuned
 *  to match comfortable prose reading speed (~200 WPM → ~25 px/s with
 *  the current line-height). Slower = more relaxed; the trade-off is
 *  long tooltips stay open longer. */
const AUTO_SCROLL_PX_PER_SEC = 24;
/** How long the user gets to read the top of the block before the
 *  auto-scroll kicks in. Without this pause the first line would be
 *  lost every time — people need a beat to orient. */
const AUTO_SCROLL_INITIAL_DELAY_MS = 1200;
/** When the scroll reaches the bottom, hold there before easing back
 *  to the top (loop), so slow readers can finish the last paragraph. */
const AUTO_SCROLL_HOLD_AT_BOTTOM_MS = 2500;

/** Floating tooltip that mirrors the content of the click-to-open
 *  `.disc-badge-info-popover` from ChatHeader, but on hover. Shows the
 *  profile's avatar + identity + role + a truncated `persona_prompt`
 *  so users understand what the profile will actually DO before they
 *  select it.
 *
 *  Renders into `document.body` via a portal so the tooltip isn't
 *  clipped by parent overflow-hidden containers (which many chip
 *  lists have). Position is recomputed on every open so dynamic
 *  layouts (sidebar collapse, resize) don't leave it off-screen.
 *
 *  The tooltip itself is inert to mouse input (`pointer-events: none`)
 *  — users who want the full prompt click the chip to open the
 *  existing popover, which has proper interactive content. */
export function ProfileTooltip({
  profile,
  children,
  maxPromptChars = 800,
  openDelayMs = 300,
}: ProfileTooltipProps) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const triggerRef = useRef<HTMLSpanElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const openTimerRef = useRef<number | null>(null);

  // Auto-scroll the persona_prompt container when the tooltip opens,
  // so users who hover a long prompt can read the whole thing without
  // needing to move the mouse (which would close the tooltip). Loops:
  // scroll to bottom, hold a beat, ease back to top, hold a beat, repeat.
  useEffect(() => {
    if (!open) return;
    const el = contentRef.current;
    if (!el) return;
    // Honor reduced-motion: if the user disabled animations, just hold
    // at the top. They can still click the chip to open the full popover.
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;

    el.scrollTop = 0;
    const overflowPx = el.scrollHeight - el.clientHeight;
    if (overflowPx <= 1) return;

    const scrollDurMs = (overflowPx / AUTO_SCROLL_PX_PER_SEC) * 1000;
    let raf = 0;
    let cancelled = false;

    const runLoop = () => {
      if (cancelled) return;
      // Phase 1 — wait at top
      const phase1Start = performance.now() + AUTO_SCROLL_INITIAL_DELAY_MS;
      // Phase 2 — scroll down
      const phase2End = phase1Start + scrollDurMs;
      // Phase 3 — hold at bottom
      const phase3End = phase2End + AUTO_SCROLL_HOLD_AT_BOTTOM_MS;
      // Phase 4 — ease back to top (half the scroll time — faster reset)
      const phase4End = phase3End + scrollDurMs * 0.5;
      // Phase 5 — hold at top, then loop
      const phase5End = phase4End + AUTO_SCROLL_INITIAL_DELAY_MS;

      const tick = (now: number) => {
        if (cancelled) return;
        if (now < phase1Start) {
          el.scrollTop = 0;
        } else if (now < phase2End) {
          const t = (now - phase1Start) / scrollDurMs;
          el.scrollTop = overflowPx * t;
        } else if (now < phase3End) {
          el.scrollTop = overflowPx;
        } else if (now < phase4End) {
          const t = (now - phase3End) / (scrollDurMs * 0.5);
          el.scrollTop = overflowPx * (1 - t);
        } else if (now < phase5End) {
          el.scrollTop = 0;
        } else {
          runLoop();
          return;
        }
        raf = requestAnimationFrame(tick);
      };
      raf = requestAnimationFrame(tick);
    };

    runLoop();

    return () => {
      cancelled = true;
      cancelAnimationFrame(raf);
    };
  }, [open]);

  // Compute position when opening — above the trigger by default,
  // flipped below if there's not enough room at the top.
  const computePosition = () => {
    const el = triggerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const TOOLTIP_MAX_WIDTH = 340;
    const VIEWPORT_PAD = 8;
    // Horizontally center on trigger, clamped to viewport.
    let left = rect.left + rect.width / 2 - TOOLTIP_MAX_WIDTH / 2;
    left = Math.max(VIEWPORT_PAD, Math.min(left, window.innerWidth - TOOLTIP_MAX_WIDTH - VIEWPORT_PAD));
    // Vertically anchor above; flip below if cramped at top.
    let top = rect.top - 8;
    const FLIP_THRESHOLD = 200;
    if (rect.top < FLIP_THRESHOLD) {
      top = rect.bottom + 8;
    }
    setPos({ top, left });
  };

  const scheduleOpen = () => {
    if (openTimerRef.current !== null) return;
    openTimerRef.current = window.setTimeout(() => {
      computePosition();
      setOpen(true);
      openTimerRef.current = null;
    }, openDelayMs);
  };

  const cancelOpen = () => {
    if (openTimerRef.current !== null) {
      window.clearTimeout(openTimerRef.current);
      openTimerRef.current = null;
    }
    setOpen(false);
  };

  // Cleanup on unmount so a pending timer doesn't call setOpen on a
  // dead component.
  useEffect(() => () => {
    if (openTimerRef.current !== null) window.clearTimeout(openTimerRef.current);
  }, []);

  const promptExcerpt = (profile.persona_prompt ?? '').slice(0, maxPromptChars);
  const promptTruncated = (profile.persona_prompt ?? '').length > maxPromptChars;

  return (
    <>
      <span
        ref={triggerRef}
        onMouseEnter={scheduleOpen}
        onMouseLeave={cancelOpen}
        onFocus={scheduleOpen}
        onBlur={cancelOpen}
        style={{ display: 'inline-flex' }}
      >
        {children}
      </span>
      {open && pos && createPortal(
        <div
          role="tooltip"
          className="profile-tooltip"
          style={{
            position: 'fixed',
            top: pos.top,
            left: pos.left,
            transform: pos.top < 0 ? 'translateY(0)' : 'translateY(-100%)',
            maxWidth: 340,
            pointerEvents: 'none',
            zIndex: 'var(--kr-z-modal)' as unknown as number,
            background: 'var(--kr-bg-elevated)',
            border: `1px solid ${profile.color}55`,
            borderRadius: 'var(--kr-r-lg)',
            padding: 'var(--kr-sp-6) var(--kr-sp-7)',
            boxShadow: 'var(--kr-shadow-popover)',
            fontSize: 'var(--kr-fs-sm)',
            color: 'var(--kr-text-primary)',
            lineHeight: 1.45,
          }}
        >
          {/* Header — avatar + name + role */}
          <div style={{ display: 'flex', gap: 'var(--kr-sp-4)', alignItems: 'center', marginBottom: 'var(--kr-sp-4)' }}>
            <span style={{ fontSize: 18, lineHeight: 1 }}>{profile.avatar}</span>
            <div style={{ display: 'flex', flexDirection: 'column', minWidth: 0 }}>
              <span style={{ fontWeight: 600, color: profile.color }}>
                {profile.persona_name || profile.name}
              </span>
              <span style={{ fontSize: 'var(--kr-fs-xs)', color: 'var(--kr-text-muted)' }}>
                {profile.role}
              </span>
            </div>
          </div>

          {/* Persona excerpt — auto-scrolled on open so the user can
              read the whole thing without moving the mouse (which
              would close the tooltip). See the useEffect above. */}
          {promptExcerpt && (
            <div
              ref={contentRef}
              style={{
                color: 'var(--kr-text-secondary)',
                whiteSpace: 'pre-wrap',
                fontSize: 'var(--kr-fs-sm)',
                maxHeight: 180,
                overflow: 'hidden',
                position: 'relative',
              }}
            >
              {promptExcerpt}
              {promptTruncated && (
                <span style={{ color: 'var(--kr-text-muted)', fontStyle: 'italic' }}> …</span>
              )}
            </div>
          )}
        </div>,
        document.body,
      )}
    </>
  );
}
