import { useLayoutEffect, useRef } from 'react';
import { ToastItem } from './ToastItem';
import type { Toast } from './toastTypes';
import './toastStack.css';

/** KT-1883 — placement in toastStack.css: bottom left from --kr-bp-md up,
 *  under the app's top bar below it; newest at the bottom. The stack is
 *  capped to the window and scrolls, so each new toast is scrolled into view;
 *  a re-render that adds nothing leaves the user's scroll alone. */
export function ToastStack({ toasts, onDismiss }: { toasts: readonly Toast[]; onDismiss: (id: number) => void }) {
  const stack = useRef<HTMLDivElement>(null);
  const newest = toasts.length ? toasts[toasts.length - 1].id : null;
  useLayoutEffect(() => {
    const el = stack.current;
    if (el && newest !== null) el.scrollTop = el.scrollHeight;
  }, [newest]);
  return (
    <div className="kr-toast-stack" ref={stack}>
      {toasts.map(item => (
        <ToastItem key={item.id} toast={item} onDismiss={() => onDismiss(item.id)} />
      ))}
    </div>
  );
}
