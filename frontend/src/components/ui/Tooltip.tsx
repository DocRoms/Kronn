import {
  cloneElement, useCallback, useEffect, useId, useRef, useState,
  type FocusEvent, type ReactElement, type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
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
 *  moving focus (WCAG 1.4.13). */
export function Tooltip({ content, children, placement = 'top', describe = true }: TooltipProps) {
  const id = useId();
  const anchorRef = useRef<HTMLSpanElement>(null);
  const tipRef = useRef<HTMLSpanElement>(null);
  const closeTimer = useRef<number | null>(null);
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

  // Escape closes it wherever focus is: a tooltip opened by hover too.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') hide(); };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open, hide]);

  useEffect(() => cancelClose, [cancelClose]);

  const onFocus = (e: FocusEvent<HTMLSpanElement>) => {
    // A click focuses a button too; only keyboard focus opens the tooltip.
    if (focusVisible(e.target)) show();
  };
  const onBlur = (e: FocusEvent<HTMLSpanElement>) => {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) hide();
  };

  const describedBy = [children.props['aria-describedby'], describe && open ? id : null]
    .filter(Boolean).join(' ') || undefined;

  return (
    <span
      ref={anchorRef}
      className="ui-tooltip-anchor"
      onMouseEnter={show}
      onMouseLeave={hideSoon}
      onFocus={onFocus}
      onBlur={onBlur}
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
          onMouseEnter={cancelClose}
          onMouseLeave={hideSoon}
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
