import { useState, useEffect, useCallback, useRef } from 'react';

interface PositionResult {
  spotlight: React.CSSProperties | null;
  tooltip: React.CSSProperties;
  position: 'top' | 'bottom' | 'left' | 'right' | 'center';
}

const PADDING = 8;       // gap around the target for the spotlight
const TOOLTIP_GAP = 12;  // gap between spotlight and tooltip
const VIEWPORT_MARGIN = 12;

/**
 * Tracks a target element's position and computes tooltip placement.
 * Returns inline styles for the spotlight and tooltip divs.
 */
export function useTourPositioning(
  selector: string | null,
  preferredPosition?: 'top' | 'bottom' | 'left' | 'right',
  isMobile = false,
  pulse = false,
  /** Optional secondary selector — when set, the tooltip card is
   *  positioned relative to THIS element's rect instead of the main
   *  target. Lets the spotlight pin on a tiny inner control while the
   *  tooltip sits outside the surrounding container. */
  tooltipAnchor?: string,
): PositionResult {
  const [result, setResult] = useState<PositionResult>({
    spotlight: null,
    tooltip: { top: '50%', left: '50%', transform: 'translate(-50%, -50%)' },
    position: 'center',
  });
  const prevTargetRef = useRef<HTMLElement | null>(null);

  const cleanupPrev = useCallback(() => {
    if (prevTargetRef.current) {
      prevTargetRef.current.classList.remove('tour-target-elevated', 'tour-pulse');
      prevTargetRef.current = null;
    }
  }, []);

  const measure = useCallback(() => {
    if (!selector) {
      // Centered (welcome / finale step) — clean up any previous target first
      cleanupPrev();
      setResult({
        spotlight: null,
        tooltip: { top: '50%', left: '50%', transform: 'translate(-50%, -50%)' },
        position: 'center',
      });
      return;
    }

    const el = document.querySelector<HTMLElement>(selector);
    if (!el) {
      // Target not found — clean up previous + center tooltip
      cleanupPrev();
      setResult({
        spotlight: null,
        tooltip: { top: '50%', left: '50%', transform: 'translate(-50%, -50%)' },
        position: 'center',
      });
      return;
    }

    // Manage elevation + pulse classes — clean previous if different
    if (prevTargetRef.current && prevTargetRef.current !== el) {
      prevTargetRef.current.classList.remove('tour-target-elevated', 'tour-pulse');
    }
    el.classList.add('tour-target-elevated');
    if (pulse) el.classList.add('tour-pulse');
    else el.classList.remove('tour-pulse');
    prevTargetRef.current = el;

    // Scroll into view if needed
    const rect = el.getBoundingClientRect();
    if (rect.top < 0 || rect.bottom > window.innerHeight) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      // Re-measure after scroll settles
      requestAnimationFrame(() => requestAnimationFrame(() => measure()));
      return;
    }

    // Spotlight rect (with padding) — always pinned on the main target.
    const spotlight: React.CSSProperties = {
      top: rect.top - PADDING,
      left: rect.left - PADDING,
      width: rect.width + PADDING * 2,
      height: rect.height + PADDING * 2,
    };

    // Tooltip placement
    if (isMobile) {
      setResult({ spotlight, tooltip: {}, position: 'bottom' });
      return;
    }

    // The tooltip is positioned around a different rect when a
    // `tooltipAnchor` is provided — usually a parent container — so the
    // tooltip can sit OUTSIDE the container while the spotlight stays
    // on the inner target. Falls back to the target rect when the
    // anchor doesn't exist (yet), which keeps first paint sane.
    const anchorEl = tooltipAnchor
      ? document.querySelector<HTMLElement>(tooltipAnchor)
      : null;
    const anchorRect = anchorEl ? anchorEl.getBoundingClientRect() : rect;

    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const tooltipW = 340;
    const tooltipH = 200; // estimated

    const spaceTop = anchorRect.top;
    const spaceBottom = vh - anchorRect.bottom;
    const spaceLeft = anchorRect.left;
    const spaceRight = vw - anchorRect.right;

    // Try preferred, then bottom, top, right, left
    const candidates: ('bottom' | 'top' | 'right' | 'left')[] = preferredPosition
      ? [preferredPosition, 'bottom', 'top', 'right', 'left']
      : ['bottom', 'top', 'right', 'left'];

    let pos: 'top' | 'bottom' | 'left' | 'right' = 'bottom';
    for (const c of candidates) {
      if (c === 'bottom' && spaceBottom > tooltipH + TOOLTIP_GAP) { pos = 'bottom'; break; }
      if (c === 'top' && spaceTop > tooltipH + TOOLTIP_GAP) { pos = 'top'; break; }
      if (c === 'right' && spaceRight > tooltipW + TOOLTIP_GAP) { pos = 'right'; break; }
      if (c === 'left' && spaceLeft > tooltipW + TOOLTIP_GAP) { pos = 'left'; break; }
    }

    const tooltip: React.CSSProperties = {};
    const centerX = anchorRect.left + anchorRect.width / 2 - tooltipW / 2;

    switch (pos) {
      case 'bottom':
        tooltip.top = anchorRect.bottom + TOOLTIP_GAP;
        tooltip.left = Math.max(VIEWPORT_MARGIN, Math.min(centerX, vw - tooltipW - VIEWPORT_MARGIN));
        break;
      case 'top':
        tooltip.top = anchorRect.top - tooltipH - TOOLTIP_GAP;
        tooltip.left = Math.max(VIEWPORT_MARGIN, Math.min(centerX, vw - tooltipW - VIEWPORT_MARGIN));
        break;
      case 'right':
        tooltip.top = Math.max(VIEWPORT_MARGIN, anchorRect.top + anchorRect.height / 2 - tooltipH / 2);
        tooltip.left = anchorRect.right + TOOLTIP_GAP;
        break;
      case 'left':
        tooltip.top = Math.max(VIEWPORT_MARGIN, anchorRect.top + anchorRect.height / 2 - tooltipH / 2);
        tooltip.left = anchorRect.left - tooltipW - TOOLTIP_GAP;
        break;
    }

    setResult({ spotlight, tooltip, position: pos });
  }, [selector, preferredPosition, isMobile, pulse, cleanupPrev, tooltipAnchor]);

  useEffect(() => {
    measure();
    window.addEventListener('resize', measure);
    window.addEventListener('scroll', measure, true);
    return () => {
      window.removeEventListener('resize', measure);
      window.removeEventListener('scroll', measure, true);
      cleanupPrev();
    };
  }, [measure]);

  return result;
}

/**
 * Wait for a CSS selector to appear in the DOM (max 2s).
 * Uses MutationObserver for efficiency.
 */
export function waitForElement(selector: string, timeout = 2000): Promise<HTMLElement | null> {
  return new Promise((resolve) => {
    const existing = document.querySelector<HTMLElement>(selector);
    if (existing) { resolve(existing); return; }

    const timer = setTimeout(() => {
      observer.disconnect();
      resolve(null);
    }, timeout);

    const observer = new MutationObserver(() => {
      const el = document.querySelector<HTMLElement>(selector);
      if (el) {
        clearTimeout(timer);
        observer.disconnect();
        resolve(el);
      }
    });

    observer.observe(document.body, { childList: true, subtree: true });
  });
}
