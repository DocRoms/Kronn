import { useRef, type ReactNode } from 'react';
import { useT } from '../../lib/I18nContext';
import { Button } from './Button';
import type { NonEmptyLabel } from './IconButton';
import { Dialog, type DialogSize } from './Dialog';
import './ConfirmDialog.css';

/** danger: the action deletes or loses something; primary: it changes
 *  something and loses nothing. */
export type ConfirmTone = 'danger' | 'primary';

export interface ConfirmDialogProps<L extends string = string> {
  open: boolean;
  /** The question, which names the dialog: "Delete 3 projects?". */
  title: ReactNode;
  /** What happens, and what does not; line breaks are kept. */
  message?: ReactNode;
  /** The action, with the verb and the object: "Delete 3 projects", never
   *  "OK" or "Yes". Required and not empty. */
  confirmLabel: NonEmptyLabel<L>;
  /** Defaults to "Cancel". */
  cancelLabel?: string;
  tone?: ConfirmTone;
  onConfirm: () => void;
  /** Cancel, Escape, the cross and the scrim. */
  onCancel: () => void;
  size?: DialogSize;
}

/** KT-1881 — the confirmation of an action, in place of window.confirm.
 *  An alertdialog named by its question, described by its message, the focus
 *  on Cancel (Enter right away answers no), the action last. It sits above a
 *  dialog or a drawer it is opened from. For a promise, see useConfirm. */
export function ConfirmDialog<L extends string>({
  open, title, message, confirmLabel, cancelLabel, tone = 'danger', onConfirm, onCancel, size = 'sm',
}: ConfirmDialogProps<L>) {
  const { t } = useT();
  const cancelRef = useRef<HTMLButtonElement>(null);
  return (
    <Dialog
      open={open}
      onClose={onCancel}
      title={title}
      description={message}
      role="alertdialog"
      size={size}
      layer="nested"
      className="ui-confirm-dialog"
      initialFocus={cancelRef}
      footer={(
        <>
          <Button ref={cancelRef} variant="neutral" onClick={onCancel}>{cancelLabel ?? t('ui.confirm.cancel')}</Button>
          <Button variant={tone} onClick={onConfirm}>{confirmLabel}</Button>
        </>
      )}
    />
  );
}
