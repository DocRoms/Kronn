import { useCallback, useLayoutEffect, useRef, type RefObject } from 'react';

interface ActivityPopoverOptions {
  onClose: () => void;
  triggerRef?: RefObject<HTMLButtonElement | null>;
  focusFallbackRef?: RefObject<HTMLButtonElement | null>;
}

export function useActivityPopover(options: ActivityPopoverOptions) {
  const rootRef = useRef<HTMLDivElement>(null);
  const latest = useRef(options);
  useLayoutEffect(() => { latest.current = options; });

  const closeAndRestoreFocus = useCallback(() => {
    latest.current.onClose();
    requestAnimationFrame(() => {
      const { triggerRef, focusFallbackRef } = latest.current;
      const target = triggerRef?.current?.isConnected ? triggerRef.current : focusFallbackRef?.current;
      target?.focus();
    });
  }, []);

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    let focusedWithin: Element | null = root;
    const onFocusIn = (event: FocusEvent) => {
      focusedWithin = event.target instanceof Element && root.contains(event.target) ? event.target : null;
    };
    // A live refresh can remove the focused Stop/row button. Keep keyboard
    // ownership only when that removal (not intentional outside focus) lost it.
    const contentObserver = new MutationObserver(() => {
      if (root.isConnected && focusedWithin && !focusedWithin.isConnected && document.activeElement === document.body) {
        root.focus();
      }
    });
    const position = () => {
      const anchor = latest.current.triggerRef?.current?.getBoundingClientRect();
      const margin = 8;
      const width = Math.max(0, Math.min(360, window.innerWidth - 2 * margin));
      const left = Math.max(margin, Math.min(anchor?.left ?? margin, window.innerWidth - width - margin));
      const top = Math.max(margin, Math.min((anchor?.bottom ?? 0) + 6, window.innerHeight - margin));
      Object.assign(root.style, {
        width: `${width}px`, left: `${left}px`, top: `${top}px`,
        maxHeight: `${Math.max(0, window.innerHeight - top - margin)}px`,
      });
    };
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (target instanceof Node && !root.contains(target) && !latest.current.triggerRef?.current?.contains(target)) {
        latest.current.onClose();
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || (!root.contains(document.activeElement)
        && !latest.current.triggerRef?.current?.contains(document.activeElement))) return;
      event.preventDefault();
      // The focused disclosure owns Escape, not the collection panel underneath.
      event.stopPropagation();
      closeAndRestoreFocus();
    };
    position();
    document.addEventListener('focusin', onFocusIn);
    root.focus();
    contentObserver.observe(root, { childList: true, subtree: true });
    window.addEventListener('resize', position);
    document.addEventListener('scroll', position, true);
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown, true);
    return () => {
      contentObserver.disconnect();
      document.removeEventListener('focusin', onFocusIn);
      window.removeEventListener('resize', position);
      document.removeEventListener('scroll', position, true);
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown, true);
    };
  }, [closeAndRestoreFocus]);

  return { rootRef, closeAndRestoreFocus };
}
