import { useEffect, useLayoutEffect, useRef, type RefObject } from 'react';

export interface DialogFocusOptions {
  /** The dialog is mounted and open. Defaults to true. */
  active?: boolean;
  /** Called on Escape. Omit it when the component already handles Escape. */
  onEscape?: () => void;
  /** Keep Tab inside the dialog. Defaults to true; false for non-modal inline panels. */
  trap?: boolean;
  /** Element to focus on open; defaults to the first focusable child, then the dialog. */
  initialFocus?: RefObject<HTMLElement | null>;
  /** Focus target on close when the trigger left the DOM (re-rendered in its place). */
  returnFocus?: RefObject<HTMLElement | null>;
}

const FOCUSABLE = [
  'a[href]', 'area[href]', 'button:not([disabled])', 'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])', 'textarea:not([disabled])', 'iframe', '[contenteditable="true"]',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

export function focusableIn(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE))
    .filter(el => !el.closest('[hidden], [inert], [aria-hidden="true"]'));
}

// Open dialogs, innermost last: the top one handles Tab and a modal's Escape.
const openDialogs: HTMLElement[] = [];

const isModal = (node: HTMLElement) => node.getAttribute('aria-modal') === 'true';

// Side-by-side inline panels: the innermost dialog around the target owns Escape, unless a modal opened later.
function ownsEscape(node: HTMLElement, target: Node): boolean {
  const index = openDialogs.lastIndexOf(node);
  return !openDialogs.some((other, i) => other !== node
    && ((node.contains(other) && other.contains(target)) || (i > index && isModal(other))));
}

/**
 * Initial focus, Tab trap, Escape and focus return for a dialog or drawer.
 * Attach the returned ref to the element carrying role="dialog".
 * With onEscape, Escape stops at body: a popover inside listening on document or window never sees it.
 */
export function useDialogFocus<T extends HTMLElement = HTMLDivElement>(options: DialogFocusOptions = {}): RefObject<T | null> {
  const { active = true, trap = true } = options;
  const ref = useRef<T | null>(null);
  const latest = useRef(options);
  useLayoutEffect(() => {
    latest.current = options;
  });

  useEffect(() => {
    const node = ref.current;
    if (!active || !node) return;
    const focused = document.activeElement;
    const trigger = focused instanceof HTMLElement && focused !== document.body ? focused : null;
    const fallback = latest.current.returnFocus;
    openDialogs.push(node);

    if (!node.contains(document.activeElement)) {
      const target = latest.current.initialFocus?.current ?? focusableIn(node)[0] ?? node;
      if (target === node && !node.hasAttribute('tabindex')) node.setAttribute('tabindex', '-1');
      target.focus();
    }

    const isTop = () => openDialogs[openDialogs.length - 1] === node;
    const modal = isModal(node);

    const closeOnEscape = (event: KeyboardEvent) => {
      const onEscape = latest.current.onEscape;
      // An IME composition uses Escape to cancel itself.
      if (event.key !== 'Escape' || event.isComposing || event.keyCode === 229) return;
      if (!onEscape || event.defaultPrevented) return;
      const target = event.target instanceof Node ? event.target : null;
      const inside = target !== null && node.contains(target);
      const lost = target === null || target === document || target === document.body;
      // A modal answers from the top of the stack; a non-modal only owns Escape pressed inside it.
      if (modal ? !isTop() || !(inside || lost) : !inside || !ownsEscape(node, target)) return;
      event.preventDefault();
      // One layer per Escape: document and window listeners never see it.
      event.stopPropagation();
      onEscape();
    };

    // On body: after React's root and portal handlers (an inner control can prevent it), before page listeners.
    const onBodyKeyDown = (event: KeyboardEvent) => closeOnEscape(event);

    const onWindowKeyDown = (event: KeyboardEvent) => {
      if (!isTop()) return;
      if (event.key === 'Escape') {
        // Dispatched above body (window, document): body never saw it.
        if (!(event.target instanceof Node) || event.target === document) closeOnEscape(event);
        return;
      }
      if (event.key !== 'Tab' || !trap) return;
      const current = document.activeElement;
      // Focus inside a portal popover opened from the dialog is left alone.
      const lost = current === null || current === document.body;
      if (!lost && !node.contains(current)) return;
      const items = focusableIn(node);
      if (items.length === 0) {
        event.preventDefault();
        node.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (lost) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
      } else if (event.shiftKey && (current === first || current === node)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && current === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.body.addEventListener('keydown', onBodyKeyDown);
    window.addEventListener('keydown', onWindowKeyDown);

    return () => {
      document.body.removeEventListener('keydown', onBodyKeyDown);
      window.removeEventListener('keydown', onWindowKeyDown);
      const index = openDialogs.lastIndexOf(node);
      if (index >= 0) openDialogs.splice(index, 1);
      // Another layer that now holds focus keeps it.
      const current = document.activeElement;
      if (current !== null && current !== document.body && !node.contains(current)) return;
      const back = trigger?.isConnected ? trigger : fallback?.current;
      back?.focus();
    };
  }, [active, trap]);

  return ref;
}
