import { OverlayFrame, type OverlayProps } from './overlay';
import './Drawer.css';

export type { OverlayProps as DrawerProps, OverlaySize as DrawerSize } from './overlay';

/** KT-1881 — a side panel over a scrim, the full height of the window at
 *  its end edge. The same contract as Dialog: role="dialog", aria-modal,
 *  named by its title, focus kept inside, Escape, the cross and the scrim
 *  close it, the focus goes back to the opener. Three widths (480, 640,
 *  960 px); it fills the screen under the first breakpoint. */
export function Drawer(props: OverlayProps) {
  return <OverlayFrame kind="drawer" {...props} />;
}
