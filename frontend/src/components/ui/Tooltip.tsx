import {
  cloneElement, useCallback, useEffect, useId, useRef, useState,
  type FocusEvent, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent,
  type ReactElement, type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { focusIsPlaced, layerAbove } from '../../hooks/useDialogFocus';
import { useAnchoredStyle, type Placement } from './anchor';
import './Tooltip.css';

/** How long the tooltip waits after the pointer leaves the anchor, so the
 *  pointer can move onto the tooltip itself (WCAG 1.4.13, hoverable). */
export const TOOLTIP_CLOSE_DELAY_MS = 120;

export interface TooltipProps {
  /** Text shown in the tooltip. Short: a tooltip is not a place for a paragraph. */
  content: ReactNode;
  /** One focusable element (a button, a link); it gets `aria-describedby`. */
  children: ReactElement<{ 'aria-describedby'?: string }>;
  placement?: Placement;
  /** Wire the tooltip as the anchor's description (default true). False when
   *  the tooltip repeats the anchor's accessible name, as on an IconButton. */
  describe?: boolean;
}

/** KT-1880 — replaces the native `title=`: it opens on hover AND on keyboard
 *  focus, stays while the pointer is over it, and Escape closes it without
 *  moving focus (WCAG 1.4.13). KT-2014 — when the user put it there (the
 *  pointer, or a focus the user moved) and nothing sits above it, that Escape
 *  stops there and the layer around stays open; shown by a focus a layer
 *  placed (`placeFocus`), it hides and lets the key through. Activating the
 *  anchor (Enter, Space, a press) hides it. */
export function Tooltip({ content, children, placement = 'top', describe = true }: TooltipProps) {
  const id = useId();
  const anchorRef = useRef<HTMLSpanElement>(null);
  const tipRef = useRef<HTMLSpanElement>(null);
  const closeTimer = useRef<number | null>(null);
  /** The pointer is over the anchor or the tooltip. */
  const pointerOver = useRef(false);
  /** The anchor holds a keyboard focus the user moved there (not placed by a layer). */
  const userFocus = useRef(false);
  const [open, setOpen] = useState(false);
  const { style } = useAnchoredStyle(anchorRef, tipRef, open, placement, 'center');

  const cancelClose = useCallback(() => {
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
  }, []);
  const show = useCallback(() => { cancelClose(); setOpen(true); }, [cancelClose]);
  const hide = useCallback(() => { cancelClose(); setOpen(false); }, [cancelClose]);
  const hideSoon = useCallback(() => {
    cancelClose();
    closeTimer.current = window.setTimeout(() => { closeTimer.current = null; setOpen(false); }, TOOLTIP_CLOSE_DELAY_MS);
  }, [cancelClose]);

  // Escape closes it wherever focus is: a tooltip opened by hover too. Every
  // open tooltip hides on the same Escape. KT-2014 — one the user put there,
  // with no layer above it, is the top layer: it takes that Escape (window,
  // capture, before a Dialog, a Drawer or an anchored layer around it) and
  // stops it, so that layer stays open until the next Escape. Shown by a focus
  // a layer placed, it costs no Escape: the key goes on to the layer.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      // An IME composition uses Escape to cancel itself.
      if (e.key !== 'Escape' || e.isComposing || e.keyCode === 229) return;
      const anchor = anchorRef.current;
      const owns = (pointerOver.current || userFocus.current) && anchor !== null && !layerAbove(anchor);
      hide();
      if (!owns) return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [open, hide]);

  useEffect(() => cancelClose, [cancelClose]);

  const onFocus = (e: FocusEvent<HTMLSpanElement>) => {
    // A click focuses a button too; only keyboard focus opens the tooltip.
    if (!focusVisible(e.target)) return;
    userFocus.current = !focusIsPlaced();
    show();
  };
  const onBlur = (e: FocusEvent<HTMLSpanElement>) => {
    if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
    userFocus.current = false;
    hide();
  };
  const onEnter = () => { pointerOver.current = true; show(); };
  const onLeave = () => { pointerOver.current = false; hideSoon(); };
  // Activating the anchor hides it: what it explains is now being done. The
  // tooltip is a portal but a React child: its own events are left alone.
  const fromAnchor = (target: EventTarget) => anchorRef.current?.contains(target as Node) ?? false;
  const onKeyDownCapture = (e: ReactKeyboardEvent<HTMLSpanElement>) => {
    if ((e.key === 'Enter' || e.key === ' ') && fromAnchor(e.target)) hide();
  };
  const onPointerDownCapture = (e: ReactPointerEvent<HTMLSpanElement>) => {
    if (fromAnchor(e.target)) hide();
  };

  const describedBy = [children.props['aria-describedby'], describe && open ? id : null]
    .filter(Boolean).join(' ') || undefined;

  return (
    <span
      ref={anchorRef}
      className="ui-tooltip-anchor"
      onMouseEnter={onEnter}
      onMouseLeave={onLeave}
      onFocus={onFocus}
      onBlur={onBlur}
      onKeyDownCapture={onKeyDownCapture}
      onPointerDownCapture={onPointerDownCapture}
    >
      {cloneElement(children, { 'aria-describedby': describedBy })}
      {open && createPortal(
        <span
          ref={tipRef}
          id={id}
          role="tooltip"
          className="ui-tooltip"
          style={style}
          aria-hidden={describe ? undefined : true}
          onMouseEnter={() => { pointerOver.current = true; cancelClose(); }}
          onMouseLeave={onLeave}
        >
          {content}
        </span>,
        document.body,
      )}
    </span>
  );
}

/** `:focus-visible`, or true where the selector is not supported. */
function focusVisible(el: Element): boolean {
  try {
    return el.matches(':focus-visible');
  } catch {
    return true;
  }
}
