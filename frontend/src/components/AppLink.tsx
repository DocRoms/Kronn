import type { AnchorHTMLAttributes, MouseEvent, ReactNode } from 'react';
import { isPlainLeftClick } from '../lib/newTabNavigation';

interface AppLinkProps extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href' | 'onClick'> {
  /** An address of the app, built with `lib/routes`. */
  to: string;
  /** What a plain click does, in this tab, instead of a page load. Without
   *  it the browser follows the link, which reloads the app there. */
  onNavigate?: () => void;
  children?: ReactNode;
}

/**
 * A link to an address of the app. A plain click stays in the app through
 * `onNavigate`; Ctrl/Cmd/Shift-click, the middle click and the context menu
 * are the browser's own — a new tab, a new window, "copy link address", the
 * destination in the status bar — because the element is a real link.
 *
 * Not React Router's `<Link>`: pages are rendered, and tested, outside any
 * router, and get their navigation from the route that owns them.
 */
export function AppLink({ to, onNavigate, children, ...rest }: AppLinkProps) {
  const handleClick = (event: MouseEvent<HTMLAnchorElement>) => {
    if (!onNavigate || !isPlainLeftClick(event)) return;
    event.preventDefault();
    onNavigate();
  };
  return <a {...rest} href={to} onClick={handleClick}>{children}</a>;
}
