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

// Open dialogs, innermost last: the top one handles Tab and the Escape of a lost focus.
const openDialogs: HTMLElement[] = [];

// KT-2014 — every open floating layer (the dialogs above, and the layers that
// manage their own focus, such as a Menu), in opening order. A layer opened
// while another one is open was opened from it: a press inside the later one
// is not a press outside the earlier one.
const openLayers: HTMLElement[] = [];

/** Put `node` on the stack of open layers; the returned function takes it off.
 *  Dialogs, anchored layers and Menu call it. Any other popover portalled to
 *  `body` (a picker) must call it while it is open to count as opened from the
 *  layer under it; otherwise a press in it closes that anchored layer. */
export function registerLayer(node: HTMLElement): () => void {
  openLayers.push(node);
  return () => {
    const index = openLayers.lastIndexOf(node);
    if (index >= 0) openLayers.splice(index, 1);
  };
}

/** `target` sits in a layer on the stack opened after `node` (a Menu, a
 *  dialog or another anchored layer opened from it), even when that layer is
 *  a portal elsewhere. A popover that never called `registerLayer` is not seen. */
export function insideLayerAbove(node: HTMLElement, target: Node): boolean {
  const index = openLayers.lastIndexOf(node);
  return index >= 0 && openLayers.some((layer, i) => i > index && layer.contains(target));
}

/** `target` lies under a layer opened after the one that holds it (or, on
 *  the page, under any open layer): something else sits on top of it. A layer
 *  nested in the DOM of the one holding `target` (an inline panel) is not above. */
export function layerAbove(target: Node): boolean {
  let holder = -1;
  openLayers.forEach((layer, i) => { if (layer.contains(target)) holder = i; });
  const own = holder >= 0 ? openLayers[holder] : null;
  return openLayers.some((layer, i) => i > holder && !(own?.contains(layer)));
}

// KT-2014 — a focus a layer places (its initial focus, the focus it gives back
// on close) is not a focus the user moved: a Tooltip it shows owns no Escape.
let placing = 0;

/** Focus `el` on behalf of a layer (open, close), not of the user. */
export function placeFocus(el: HTMLElement | null | undefined, options?: FocusOptions): void {
  if (!el) return;
  placing += 1;
  try {
    el.focus(options);
  } finally {
    placing -= 1;
  }
}

/** True while `placeFocus` runs: the focus event under way was placed by a layer. */
export function focusIsPlaced(): boolean {
  return placing > 0;
}

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
 * An open Tooltip the user put there takes Escape before it (window, capture): one layer per Escape.
 * Its initial focus and the focus it gives back are placed (`placeFocus`): a tooltip they show costs no Escape.
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
    const unregister = registerLayer(node);

    if (!node.contains(document.activeElement)) {
      const target = latest.current.initialFocus?.current ?? focusableIn(node)[0] ?? node;
      if (target === node && !node.hasAttribute('tabindex')) node.setAttribute('tabindex', '-1');
      placeFocus(target);
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
      const active = document.activeElement;
      const focusLost = active === null || active === document.body;
      // A modal answers from the top of the stack; a non-modal owns Escape pressed inside it, and,
      // on top of the stack, the Escape of a lost focus (its focused control was removed) (KT-2014).
      const owns = modal
        ? isTop() && (inside || lost)
        : (inside && ownsEscape(node, target as Node)) || (lost && focusLost && isTop());
      if (!owns) return;
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
      unregister();
      // Another layer that now holds focus keeps it.
      const current = document.activeElement;
      if (current !== null && current !== document.body && !node.contains(current)) return;
      const back = trigger?.isConnected ? trigger : fallback?.current;
      placeFocus(back);
    };
  }, [active, trap]);

  return ref;
}
