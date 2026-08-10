import { useState, useEffect, useCallback, useRef } from 'react';

interface PositionResult {
  spotlight: React.CSSProperties | null;
  secondarySpotlights: React.CSSProperties[];
  tooltip: React.CSSProperties;
  position: 'top' | 'bottom' | 'left' | 'right' | 'center';
}

const PADDING = 8;       // gap around the target for the spotlight
const TOOLTIP_GAP = 12;  // gap between spotlight and tooltip
const VIEWPORT_MARGIN = 12;
const EMPTY_SECONDARY_SELECTORS: string[] = [];

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
  /** Identifies the current step. Two consecutive steps can share a selector —
   *  the form card, for instance — and without this the effect never re-runs, so
   *  a longer description keeps the previous card's measured height and the clamp
   *  lets it hang past the bottom edge. */
  stepKey?: string,
  secondarySelectors: string[] = EMPTY_SECONDARY_SELECTORS,
): PositionResult {
  const [result, setResult] = useState<PositionResult>({
    spotlight: null,
    secondarySpotlights: [],
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
    // The step identity intentionally invalidates this measurement even when
    // consecutive steps share the same selector: their tooltip size can differ.
    void stepKey;

    if (!selector) {
      // Centered (welcome / finale step) — clean up any previous target first
      cleanupPrev();
      setResult({
        spotlight: null,
        secondarySpotlights: [],
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
        secondarySpotlights: [],
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
    let rect = el.getBoundingClientRect();
    const targetFitsViewport = rect.height <= window.innerHeight;
    const isOutsideViewport = rect.bottom <= 0 || rect.top >= window.innerHeight;
    const isPartlyClipped = targetFitsViewport
      && (rect.top < 0 || rect.bottom > window.innerHeight);
    if (isOutsideViewport || isPartlyClipped) {
      // An instant scroll gives the spotlight and target one authoritative
      // rectangle. Smooth scrolling left the highlight hundreds of pixels
      // behind the Agents accordion on short windows. Tall page containers are
      // allowed to intersect the viewport without an impossible attempt to fit
      // their entire height.
      el.scrollIntoView({ behavior: 'auto', block: 'center' });
      rect = el.getBoundingClientRect();
    }

    // A target can exist and still have no box — `display: contents` is the
    // common case. Highlighting it produced a padding-sized square in the corner
    // instead of a highlight, so treat it like a missing target: no spotlight,
    // centered card, and the backdrop dims itself.
    if (rect.width === 0 || rect.height === 0) {
      cleanupPrev();
      setResult({
        spotlight: null,
        secondarySpotlights: [],
        tooltip: { top: '50%', left: '50%', transform: 'translate(-50%, -50%)' },
        position: 'center',
      });
      return;
    }

    // Spotlight rect (with padding) — always pinned on the main target.
    const spotlight: React.CSSProperties = {
      top: rect.top - PADDING,
      left: rect.left - PADDING,
      width: rect.width + PADDING * 2,
      height: rect.height + PADDING * 2,
    };
    const secondarySpotlights = secondarySelectors.flatMap((secondarySelector) => {
      const secondary = document.querySelector<HTMLElement>(secondarySelector);
      if (!secondary) return [];
      const secondaryRect = secondary.getBoundingClientRect();
      if (
        secondaryRect.width === 0
        || secondaryRect.height === 0
        || secondaryRect.bottom < 0
        || secondaryRect.top > window.innerHeight
      ) return [];
      return [{
        top: secondaryRect.top - PADDING,
        left: secondaryRect.left - PADDING,
        width: secondaryRect.width + PADDING * 2,
        height: secondaryRect.height + PADDING * 2,
      } satisfies React.CSSProperties];
    });

    // Tooltip placement
    if (isMobile) {
      setResult({ spotlight, secondarySpotlights, tooltip: {}, position: 'bottom' });
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
    // Measure the card already on screen rather than trusting an estimate: a
    // long description makes it much taller than 200 px, and clamping against a
    // wrong height is how it ends up half off-screen. Falls back to the estimate
    // on the very first paint, then self-corrects on the next measure.
    const card = document.querySelector<HTMLElement>('.tour-tooltip');
    const tooltipW = card?.offsetWidth || 340;
    const tooltipH = card?.offsetHeight || 200;

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

    // Clamp BOTH axes. The loop above only breaks when a side has room, so a
    // target as tall as the viewport — a page container, for instance — left
    // `pos` at its default 'bottom' and pushed the card below the fold: the tour
    // became unfinishable because its only buttons were off-screen. Only the
    // horizontal axis was clamped, which is why this went unnoticed.
    const clamp = (value: number, size: number, viewport: number) =>
      Math.max(VIEWPORT_MARGIN, Math.min(value, viewport - size - VIEWPORT_MARGIN));
    tooltip.top = clamp(Number(tooltip.top ?? VIEWPORT_MARGIN), tooltipH, vh);
    tooltip.left = clamp(Number(tooltip.left ?? VIEWPORT_MARGIN), tooltipW, vw);

    setResult({ spotlight, secondarySpotlights, tooltip, position: pos });
  }, [
    selector,
    preferredPosition,
    isMobile,
    pulse,
    cleanupPrev,
    tooltipAnchor,
    stepKey,
    secondarySelectors,
  ]);

  useEffect(() => {
    let trackedTarget: HTMLElement | null = null;
    let trackedGeometry = '';
    let layoutTrackingFrame = 0;

    const readTargetGeometry = () => {
      if (!selector) return { target: null, geometry: '' };
      const target = document.querySelector<HTMLElement>(selector);
      if (!target) return { target: null, geometry: 'missing' };
      const rect = target.getBoundingClientRect();
      return {
        target,
        geometry: `${rect.top}:${rect.left}:${rect.width}:${rect.height}`,
      };
    };

    // A target can move without emitting scroll or resize. This happens on
    // data-heavy pages when an API response fills a section above the current
    // coachmark: the target's own size is unchanged, but its viewport position
    // is not. Track the actual rect while the tour is visible and only remeasure
    // when it changes, so the spotlight cannot remain pinned to stale geometry.
    const trackTargetLayout = () => {
      const current = readTargetGeometry();
      if (current.target !== trackedTarget || current.geometry !== trackedGeometry) {
        trackedTarget = current.target;
        trackedGeometry = current.geometry;
        measure();
      }
      layoutTrackingFrame = requestAnimationFrame(trackTargetLayout);
    };

    const initialMeasureFrame = requestAnimationFrame(() => {
      measure();
      const current = readTargetGeometry();
      trackedTarget = current.target;
      trackedGeometry = current.geometry;
      layoutTrackingFrame = requestAnimationFrame(trackTargetLayout);
    });
    window.addEventListener('resize', measure);
    window.addEventListener('scroll', measure, true);
    return () => {
      cancelAnimationFrame(initialMeasureFrame);
      cancelAnimationFrame(layoutTrackingFrame);
      window.removeEventListener('resize', measure);
      window.removeEventListener('scroll', measure, true);
      cleanupPrev();
    };
  }, [cleanupPrev, measure, selector]);

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
