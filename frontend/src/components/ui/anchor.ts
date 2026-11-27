// KT-1880 — where a floating layer (Tooltip, Menu) sits next to its anchor.
//
// The layer is rendered into document.body with `position: fixed`, so a list
// with `overflow: hidden` or its own stacking context does not clip it. Its
// place is computed from the anchor's box AND the layer's own size, once the
// layer is rendered, and again on scroll and resize while it stays open. It
// never leaves the window: it flips to the side with more room, is clamped
// sideways, and a layer taller than the room scrolls inside a max-height.
import { useLayoutEffect, useState, type CSSProperties, type RefObject } from 'react';

export type Placement = 'top' | 'bottom';
export type Align = 'center' | 'start' | 'end';

/** Gap between the anchor and the layer, in px (= --kr-space-1). */
export const ANCHOR_GAP = 4;
/** Distance the layer keeps from the window's edges, in px (= --kr-space-2). */
export const VIEWPORT_MARGIN = 8;

export interface AnchorBox { top: number; bottom: number; left: number; right: number }
export interface Size { width: number; height: number }

/** Fixed-position style of a layer of size `layer` next to `anchor`, inside a
 *  window of size `viewport`.
 *  - Vertically: the asked side, unless the layer does not fit there and the
 *    other side has more room. A layer that fits on neither side keeps the
 *    better one and gets its room as `maxHeight`.
 *  - Horizontally: lined up as asked, then clamped so that it stays
 *    `VIEWPORT_MARGIN` from both edges; a layer wider than the window is
 *    pinned to the left margin. */
export function anchoredStyle(
  anchor: AnchorBox, layer: Size, viewport: Size, placement: Placement, align: Align,
): CSSProperties {
  const room: Record<Placement, number> = {
    bottom: viewport.height - VIEWPORT_MARGIN - (anchor.bottom + ANCHOR_GAP),
    top: anchor.top - ANCHOR_GAP - VIEWPORT_MARGIN,
  };
  const other: Placement = placement === 'top' ? 'bottom' : 'top';
  const side = layer.height > room[placement] && room[other] > room[placement] ? other : placement;
  const fits = layer.height <= room[side];
  const height = fits ? layer.height : Math.max(0, room[side]);

  const top = side === 'bottom' ? anchor.bottom + ANCHOR_GAP : anchor.top - ANCHOR_GAP - height;

  let left = align === 'center'
    ? (anchor.left + anchor.right) / 2 - layer.width / 2
    : align === 'start' ? anchor.left : anchor.right - layer.width;
  left = Math.min(left, viewport.width - VIEWPORT_MARGIN - layer.width);
  left = Math.max(left, VIEWPORT_MARGIN);

  const style: CSSProperties = { position: 'fixed', top, left };
  if (!fits) style.maxHeight = height;
  return style;
}

/** Style of a layer before it is measured: out of sight, at the window's
 *  origin, so its width is not squeezed by a previous place. */
const UNPLACED: CSSProperties = { position: 'fixed', top: 0, left: 0, visibility: 'hidden' };

export interface Anchored {
  style: CSSProperties;
  /** False while the layer waits to be measured, hidden: a browser refuses
   *  focus on it until then, so focus a child only once this is true. */
  placed: boolean;
}

/** The style of the layer `layerRef` anchored to `anchorRef`, kept in place
 *  while `open`. The layer is measured in a layout effect, before the browser
 *  paints, so it never shows at a wrong place. */
export function useAnchoredStyle(
  anchorRef: RefObject<HTMLElement | null>,
  layerRef: RefObject<HTMLElement | null>,
  open: boolean,
  placement: Placement,
  align: Align,
): Anchored {
  const [style, setStyle] = useState<CSSProperties>(UNPLACED);
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const anchor = anchorRef.current;
      const layer = layerRef.current;
      if (!anchor || !layer) return;
      // The natural height, not the one a previous max-height cut down:
      // scrollHeight is the content and padding, plus the borders.
      const size = {
        width: layer.offsetWidth,
        height: layer.scrollHeight + layer.offsetHeight - layer.clientHeight,
      };
      const root = document.documentElement;
      const viewport = {
        width: root.clientWidth || window.innerWidth,
        height: root.clientHeight || window.innerHeight,
      };
      setStyle(anchoredStyle(anchor.getBoundingClientRect(), size, viewport, placement, align));
    };
    place();
    window.addEventListener('scroll', place, true);
    window.addEventListener('resize', place);
    return () => {
      window.removeEventListener('scroll', place, true);
      window.removeEventListener('resize', place);
      setStyle(UNPLACED);
    };
  }, [anchorRef, layerRef, open, placement, align]);
  return { style, placed: style !== UNPLACED };
}
