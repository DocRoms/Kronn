// KT-1936 — a non-modal floating layer opened from a control: a list or a
// details card rendered in document.body (so a header that clips or scrolls
// cannot cut it) and placed next to its anchor by `useAnchoredStyle`.
//
// Its keyboard and pointer contract, the same for every such layer:
// - on open, once the layer is placed (a hidden layer refuses focus), focus
//   moves to its first control;
// - Tab and Shift+Tab move between its controls and stay inside it;
// - Escape closes it and gives focus back to the anchor that opened it;
// - a press outside the anchor and the layer closes it, and focus stays where
//   the press lands; a press inside a layer opened from it that is on the
//   stack of open layers (a Menu, a useDialogFocus dialog, another anchored
//   layer) is not outside (KT-2014). A portal popover that does not call
//   `registerLayer` (AgentSwitchPicker, DecisionModelPicker today) is outside.
import { useCallback, useEffect, useRef, type RefCallback, type RefObject } from 'react';
import { insideLayerAbove, placeFocus, useDialogFocus } from '../../hooks/useDialogFocus';
import { useAnchoredStyle, type Align, type Anchored, type Placement } from './anchor';

export interface AnchoredLayerOptions {
  /** The control that opens the layer: its place, and where focus returns. */
  anchorRef: RefObject<HTMLElement | null>;
  open: boolean;
  placement: Placement;
  align: Align;
  /** Escape or a press outside: the caller closes the layer. */
  onClose: () => void;
}

export interface AnchoredLayer<T extends HTMLElement> extends Anchored {
  /** Put on the layer's root (the element with role="dialog"). */
  ref: RefCallback<T>;
}

export function useAnchoredLayer<T extends HTMLElement>({
  anchorRef, open, placement, align, onClose,
}: AnchoredLayerOptions): AnchoredLayer<T> {
  const layerRef = useRef<T | null>(null);
  const anchored = useAnchoredStyle(anchorRef, layerRef, open, placement, align);
  const latestClose = useRef(onClose);
  useEffect(() => {
    latestClose.current = onClose;
  });

  const focusRef = useDialogFocus<T>({
    active: open && anchored.placed,
    trap: true,
    returnFocus: anchorRef,
    onEscape: () => {
      latestClose.current();
      // Back to the anchor, whatever had focus when the layer opened (a row
      // of another list that is gone, for instance). Placed: the anchor's
      // tooltip costs no Escape.
      placeFocus(anchorRef.current);
    },
  });

  const ref = useCallback<RefCallback<T>>(node => {
    layerRef.current = node;
    focusRef.current = node;
  }, [focusRef]);

  // A press outside the anchor and the layer closes it. The anchor is left to
  // its own click, which toggles the layer.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (!target) return;
      const layer = layerRef.current;
      if (layer?.contains(target) || anchorRef.current?.contains(target)) return;
      if (layer && insideLayerAbove(layer, target)) return;
      latestClose.current();
    };
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [open, anchorRef]);

  return { ...anchored, ref };
}
