import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { ConfirmDialog, type ConfirmTone } from './ConfirmDialog';

export interface ConfirmRequest {
  /** The question: "Delete version v3?". */
  title: string;
  /** What happens, and what does not. */
  message?: ReactNode;
  /** The verb and the object: "Delete v3", never "OK". */
  confirmLabel: string;
  cancelLabel?: string;
  tone?: ConfirmTone;
}

interface Pending extends ConfirmRequest {
  resolve: (answer: boolean) => void;
}

/** KT-1881 — window.confirm as a ConfirmDialog: `confirm(request)` opens it
 *  and resolves true when its action is chosen, false on Cancel, Escape, the
 *  cross or the scrim (or when the screen goes away). Render `dialog` once in
 *  the component.
 *
 *    const { confirm, dialog } = useConfirm();
 *    if (!(await confirm({ title, message, confirmLabel }))) return;
 */
export function useConfirm(): { confirm: (request: ConfirmRequest) => Promise<boolean>; dialog: ReactNode } {
  const [pending, setPending] = useState<Pending | null>(null);
  const pendingRef = useRef<Pending | null>(null);

  const settle = useCallback((answer: boolean) => {
    const current = pendingRef.current;
    pendingRef.current = null;
    setPending(null);
    current?.resolve(answer);
  }, []);

  const confirm = useCallback((request: ConfirmRequest) => new Promise<boolean>(resolve => {
    // A second request answers the first one "no".
    pendingRef.current?.resolve(false);
    const next = { ...request, resolve };
    pendingRef.current = next;
    setPending(next);
  }), []);

  useEffect(() => () => {
    pendingRef.current?.resolve(false);
    pendingRef.current = null;
  }, []);

  const dialog = pending ? (
    <ConfirmDialog
      open
      title={pending.title}
      message={pending.message}
      confirmLabel={pending.confirmLabel}
      cancelLabel={pending.cancelLabel}
      tone={pending.tone}
      onConfirm={() => settle(true)}
      onCancel={() => settle(false)}
    />
  ) : null;

  return { confirm, dialog };
}
