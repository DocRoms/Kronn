import { useCallback, useEffect, useRef, useState, type RefObject } from 'react';

/**
 * KT-2017 — the band of a Settings sub-page: the row of links to its sections,
 * stuck under the app's top bar while the sub-page scrolls.
 *
 * This hook follows what the band shows and guards what it covers:
 *  - the section being read: the last one whose top has passed under the band
 *    (the last one once the bottom of the page is reached); a click on a link
 *    marks it at once and holds the mark while the scroll it starts runs;
 *  - whether the band is stuck (its head has scrolled away above it): the band
 *    then draws its line under it;
 *  - a control the Tab key reaches under the band is moved below it.
 *
 * It listens only while its sub-page is the visible one, and reads the boxes
 * once per frame at most (a scroll schedules one read; nothing is written to
 * the layout in between).
 */

/** How far below the band a section's top may still sit and count as read:
 *  an anchor lands a breath (`--kr-space-4`) below the band, plus slack. */
export const SECTION_READ_SLACK_PX = 32;
/** How long the scroll must rest before a clicked link stops holding the mark. */
export const CLICK_HOLD_IDLE_MS = 180;
/** The breath kept between the band and a control the Tab key moves below it. */
export const FOCUS_BREATH_PX = 16;

/** Layers that float over the page: a control inside one is not moved. */
const FLOATING = '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [popover]';

export function prefersReducedMotion(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** Scrolls `line` sideways, if needed, so `link` is fully in its visible box,
 *  `pad` clear of the edges (where the line fades). Only the line scrolls. */
export function revealLinkInLine(line: HTMLElement, link: HTMLElement, pad = 24): void {
  const view = line.getBoundingClientRect();
  const box = link.getBoundingClientRect();
  let delta = 0;
  if (box.left < view.left) delta = box.left - view.left - pad;
  else if (box.right > view.right) delta = box.right - view.right + pad;
  if (delta === 0) return;
  const left = Math.max(0, line.scrollLeft + delta);
  if (typeof line.scrollTo === 'function') line.scrollTo({ left, behavior: prefersReducedMotion() ? 'auto' : 'smooth' });
  else line.scrollLeft = left;
}

interface SubPageBandOptions {
  /** The band (the `nav`), a direct child of the sub-page's box. */
  band: RefObject<HTMLElement | null>;
  /** The head above it (the sub-page's name and sentence). */
  head: RefObject<HTMLElement | null>;
  /** The anchors of the sub-page's sections, in order. */
  sectionIds: readonly string[];
  /** Whether the sub-page is the visible one. */
  active: boolean;
}

export interface SubPageBand {
  /** The anchor of the section being read, or null before the first read. */
  current: string | null;
  /** Whether the band is stuck under the top bar. */
  stuck: boolean;
  /** Marks a section at once (a click on its link). */
  mark: (anchor: string) => void;
}

export function useSubPageBand({ band, head, sectionIds, active }: SubPageBandOptions): SubPageBand {
  const [current, setCurrent] = useState<string | null>(null);
  const [stuck, setStuck] = useState(false);
  // The anchor a click marked, held until the scroll it started rests.
  const held = useRef<string | null>(null);
  const holdTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const idsKey = sectionIds.join(' ');

  const release = useCallback(() => {
    if (holdTimer.current !== null) clearTimeout(holdTimer.current);
    holdTimer.current = null;
    held.current = null;
  }, []);
  const holdUntilRest = useCallback(() => {
    if (holdTimer.current !== null) clearTimeout(holdTimer.current);
    holdTimer.current = setTimeout(release, CLICK_HOLD_IDLE_MS);
  }, [release]);

  const mark = useCallback((anchor: string) => {
    held.current = anchor;
    setCurrent(anchor);
    holdUntilRest();
  }, [holdUntilRest]);

  useEffect(() => {
    if (!active) return undefined;
    const ids = idsKey ? idsKey.split(' ') : [];
    let frame = 0;

    const read = () => {
      frame = 0;
      const bandEl = band.current;
      const box = bandEl?.parentElement;
      if (!bandEl || !box) return;
      const bandRect = bandEl.getBoundingClientRect();
      const headBottom = head.current?.getBoundingClientRect().bottom ?? bandRect.top;
      setStuck(bandRect.top - headBottom > 0.5);
      if (held.current !== null) return;
      const drawn = ids.flatMap(id => {
        const el = document.getElementById(id);
        return el !== null && box.contains(el) ? [{ id, el }] : [];
      });
      if (drawn.length === 0) return;
      const page = document.scrollingElement ?? document.documentElement;
      const scrolls = page.scrollHeight > window.innerHeight + 1;
      const atBottom = scrolls && window.scrollY + window.innerHeight >= page.scrollHeight - 2;
      let next = drawn[0].id;
      if (atBottom) {
        next = drawn[drawn.length - 1].id;
      } else {
        const line = bandRect.bottom + SECTION_READ_SLACK_PX;
        for (const { id, el } of drawn) {
          if (el.getBoundingClientRect().top <= line) next = id;
        }
      }
      setCurrent(next);
    };
    const schedule = () => {
      if (frame === 0) frame = requestAnimationFrame(read);
    };
    const onScroll = () => {
      if (held.current !== null) holdUntilRest();
      schedule();
    };

    // Tab moved the focus and the browser scrolled to it, ignoring the band:
    // a control left under it is moved below it, forwards and backwards.
    const uncover = () => {
      const bandEl = band.current;
      const box = bandEl?.parentElement;
      const focused = document.activeElement;
      if (!bandEl || !box || !(focused instanceof HTMLElement)) return;
      if (!box.contains(focused) || bandEl.contains(focused) || focused.closest(FLOATING)) return;
      const bandRect = bandEl.getBoundingClientRect();
      const headBottom = head.current?.getBoundingClientRect().bottom ?? bandRect.top;
      if (bandRect.top - headBottom <= 0.5) return; // not stuck: it covers nothing
      const top = focused.getBoundingClientRect().top;
      if (top >= bandRect.bottom) return;
      window.scrollBy({ top: top - bandRect.bottom - FOCUS_BREATH_PX, behavior: 'auto' });
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Tab' || event.altKey || event.ctrlKey || event.metaKey) return;
      requestAnimationFrame(uncover);
    };

    window.addEventListener('scroll', onScroll, { passive: true });
    window.addEventListener('resize', schedule, { passive: true });
    document.addEventListener('keydown', onKeyDown);
    schedule();
    return () => {
      if (frame !== 0) cancelAnimationFrame(frame);
      window.removeEventListener('scroll', onScroll);
      window.removeEventListener('resize', schedule);
      document.removeEventListener('keydown', onKeyDown);
      release();
    };
  }, [active, band, head, holdUntilRest, idsKey, release]);

  return { current, stuck, mark };
}
