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

// Open dialogs, innermost last: only the top one handles Escape and Tab.
const openDialogs: HTMLElement[] = [];

/**
 * Initial focus, Tab trap, Escape and focus return for a dialog or drawer.
 * Attach the returned ref to the element carrying role="dialog".
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

    const onKeyDown = (event: KeyboardEvent) => {
      if (openDialogs[openDialogs.length - 1] !== node) return;
      if (event.key === 'Escape') {
        const onEscape = latest.current.onEscape;
        if (onEscape && !event.defaultPrevented) {
          event.preventDefault();
          onEscape();
        }
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
    window.addEventListener('keydown', onKeyDown);

    return () => {
      window.removeEventListener('keydown', onKeyDown);
      const index = openDialogs.lastIndexOf(node);
      if (index >= 0) openDialogs.splice(index, 1);
      const back = trigger?.isConnected ? trigger : fallback?.current;
      back?.focus();
    };
  }, [active, trap]);

  return ref;
}
