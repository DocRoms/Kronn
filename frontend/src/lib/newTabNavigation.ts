import type { MouseEvent as ReactMouseEvent } from 'react';

/**
 * Opening an address of the app in a new tab, the way a browser does for a
 * link, from an element that is not one: a list row, a card, a button.
 *
 * A row keeps its element (and so its exact look) and its own plain-click
 * behaviour; only the modified clicks change: Ctrl/Cmd/Shift-click and the
 * middle click open the address in a new tab. A click on a control nested in
 * the row (a favourite star, an archive button, a checkbox) is left to it.
 *
 * Real links (`<a href>`) get all of this from the browser instead, with the
 * context menu and the URL preview: prefer them where the element allows it.
 */

const NESTED_CONTROL = 'a[href], button, input, select, textarea, label, [role="button"], [role="checkbox"], [role="switch"], [role="menuitem"]';

/** Whether a click is the plain left click a link follows in the same tab. */
export function isPlainLeftClick(event: Pick<MouseEvent, 'button' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey'>): boolean {
  return event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
}

function onNestedControl(event: ReactMouseEvent<HTMLElement>): boolean {
  const control = (event.target as Element | null)?.closest?.(NESTED_CONTROL);
  return Boolean(control && control !== event.currentTarget && event.currentTarget.contains(control));
}

export function openInNewTab(path: string, open: (url: string, target: string, features: string) => unknown = window.open.bind(window)): void {
  open(new URL(path, window.location.origin).href, '_blank', 'noopener,noreferrer');
}

export interface NewTabClickProps {
  onClickCapture?: (event: ReactMouseEvent<HTMLElement>) => void;
  onMouseDown?: (event: ReactMouseEvent<HTMLElement>) => void;
  onAuxClick?: (event: ReactMouseEvent<HTMLElement>) => void;
}

/**
 * Props that make an element open `path` in a new tab on a modified or
 * middle click. Spread them on the element: its own `onClick` keeps handling
 * the plain click.
 */
export function newTabClickProps(path: string | null | undefined): NewTabClickProps {
  if (!path) return {};
  return {
    onClickCapture: (event: ReactMouseEvent<HTMLElement>) => {
      if (event.button !== 0 || !(event.ctrlKey || event.metaKey || event.shiftKey) || onNestedControl(event)) return;
      // Before the element's own click handler: it must not also run.
      event.preventDefault();
      event.stopPropagation();
      openInNewTab(path);
    },
    onMouseDown: (event: ReactMouseEvent<HTMLElement>) => {
      // A middle press would start the browser's autoscroll instead.
      if (event.button === 1 && !onNestedControl(event)) event.preventDefault();
    },
    onAuxClick: (event: ReactMouseEvent<HTMLElement>) => {
      if (event.button !== 1 || onNestedControl(event)) return;
      event.preventDefault();
      openInNewTab(path);
    },
  };
}
