import { OverlayFrame, type OverlayProps } from './overlay';
import './Dialog.css';

export type { OverlayProps as DialogProps, OverlaySize as DialogSize } from './overlay';

/** KT-1881 — a modal dialog, centred over a scrim. role="dialog" (or
 *  "alertdialog"), aria-modal, named by its title; the focus goes to the
 *  first control of its content, Tab stays inside, Escape, the cross and the
 *  scrim close it, and the focus goes back to the opener. Three widths
 *  (480, 640, 960 px); it fills the screen under the first breakpoint. */
export function Dialog(props: OverlayProps) {
  return <OverlayFrame kind="dialog" {...props} />;
}
