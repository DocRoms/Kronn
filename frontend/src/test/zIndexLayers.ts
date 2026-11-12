// KT-1701 — vitest does not load CSS, so a computed z-index is unavailable. The
// stacking order is read from the stylesheets on disk instead, for the class
// the element really carries in the DOM.
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect } from 'vitest';
import { classZIndex } from '../styles/zIndexAudit';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..');

const zOf = (el: Element, fallbackClass: string): number => {
  const classes = [...el.classList];
  const z = classes.map(c => classZIndex(c, SRC)).find((v): v is number => v !== null);
  expect(z, `no z-index found in the stylesheets for .${classes.join('.')} (${fallbackClass})`).toBeTypeOf('number');
  return z!;
};

/**
 * The picker popover is portaled to <body>, so the dialog it was opened from
 * does not contain it: it has to win on z-index. `opener` is the picker
 * trigger inside the dialog; `overlayClass` the dialog's backdrop class.
 */
export function expectPopoverAboveOverlay(opener: Element, popover: Element, overlayClass: string): void {
  const overlay = opener.closest(`.${overlayClass}`);
  expect(overlay, `the picker trigger is not inside .${overlayClass}`).not.toBeNull();
  expect(overlay!.contains(popover), 'the popover is expected in a body portal, outside the dialog').toBe(false);
  expect(zOf(popover, 'popover')).toBeGreaterThan(zOf(overlay!, 'overlay'));
}
