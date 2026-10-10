/**
 * Bringing an anchor of a long page into view once the page has settled.
 *
 * Scrolling the moment the element exists lands too high on a page whose
 * content above it is still loading: every section that renders afterwards
 * pushes the target down. So this waits for the target to be there and ready
 * (a form field: enabled), and for its position to stop moving, then scrolls
 * to it: to the top for a section, to the centre with the focus for a field.
 * Content that still arrives above it afterwards is followed for a short,
 * bounded while, until the reader scrolls on their own.
 */

/** How long a target may take to appear and settle before it is revealed anyway. */
const MAX_WAIT_MS = 3000;
/** How long its position must hold still to count as settled. */
const STABLE_MS = 250;
/** How long after the reveal a layout shift above it is still corrected. */
const FOLLOW_MS = 1500;
/** A shift smaller than this is not worth a correction. */
const TOLERANCE_PX = 4;

const FIELD = 'input, select, textarea, button';

function isField(element: Element): element is HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement | HTMLButtonElement {
  return element.matches(FIELD);
}

function isReady(element: Element): boolean {
  return !isField(element) || !element.disabled;
}

/**
 * Reveal the element with this id once it is ready and its position holds.
 * Returns a cancel function (an effect's cleanup).
 */
export function revealWhenSettled(id: string, now: () => number = () => performance.now()): () => void {
  let cancelled = false;
  let frame = 0;
  const started = now();
  let lastTop: number | null = null;
  let stableSince = 0;
  let revealedAt = 0;
  let alignedTop: number | null = null;

  const stopOnReader = () => { cancelled = true; };
  const readerEvents = ['wheel', 'touchstart', 'keydown', 'pointerdown'] as const;
  readerEvents.forEach(type => window.addEventListener(type, stopOnReader, { passive: true, capture: true }));
  const release = () => {
    cancelAnimationFrame(frame);
    readerEvents.forEach(type => window.removeEventListener(type, stopOnReader, { capture: true }));
  };

  const scrollTo = (element: HTMLElement, behavior: ScrollBehavior) => {
    element.scrollIntoView?.({ behavior, block: isField(element) ? 'center' : 'start' });
  };

  const tick = () => {
    if (cancelled) { release(); return; }
    const element = document.getElementById(id);
    const elapsed = now() - started;
    if (!revealedAt) {
      if (element && isReady(element)) {
        const top = element.getBoundingClientRect().top;
        if (lastTop === null || Math.abs(top - lastTop) > TOLERANCE_PX) {
          lastTop = top;
          stableSince = now();
        }
        if (now() - stableSince >= STABLE_MS || elapsed >= MAX_WAIT_MS) {
          scrollTo(element, 'smooth');
          if (isField(element)) element.focus({ preventScroll: true });
          revealedAt = now();
        }
      } else if (elapsed >= MAX_WAIT_MS) {
        release();
        return;
      }
    } else if (element) {
      // Once the smooth scroll has ended, remember where the target stands;
      // if content above it then pushes it away, put it back, instantly.
      const top = element.getBoundingClientRect().top;
      if (now() - revealedAt < 600) {
        alignedTop = top;
      } else if (alignedTop !== null && Math.abs(top - alignedTop) > TOLERANCE_PX) {
        scrollTo(element, 'auto');
        alignedTop = element.getBoundingClientRect().top;
      }
      if (now() - revealedAt >= FOLLOW_MS) { release(); return; }
    }
    frame = requestAnimationFrame(tick);
  };
  frame = requestAnimationFrame(tick);
  return () => { cancelled = true; release(); };
}
