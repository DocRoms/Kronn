import { useId, type MouseEvent } from 'react';
import { useT } from '../../lib/I18nContext';
import { Button } from './Button';
import './visuallyHidden.css';
import './FormFooter.css';

export interface FormFooterProps {
  /** The action, its verb and its object ("Create the workflow"). */
  submitLabel: string;
  /** Called by the action. Without it the action submits the form around it. */
  onSubmit?: () => void;
  /** Called by Cancel. Without it there is no Cancel. */
  onCancel?: () => void;
  /** Defaults to "Cancel". */
  cancelLabel?: string;
  /** What must change before the action is available, one sentence each.
   *  While there is one, the action is held and says why. */
  blockers?: readonly string[];
  /** The action is running: held, and marked busy. */
  busy?: boolean;
  /** primary (default) or danger, for a destructive action. */
  tone?: 'primary' | 'danger';
  /** Sticks to the bottom of the scrolling container (default). */
  pinned?: boolean;
  className?: string;
}

/** KT-1882 — the end of a form: what still blocks the action, then Cancel,
 *  then the action, on the right. A held action stays focusable
 *  (`aria-disabled`, not `disabled`), so the keyboard reaches it and hears
 *  why; the reasons sit in a polite live region, announced as they change. */
export function FormFooter({
  submitLabel, onSubmit, onCancel, cancelLabel, blockers = [], busy = false, tone = 'primary', pinned = true, className,
}: FormFooterProps) {
  const { t } = useT();
  const ownId = useId();
  const reasonsId = `${ownId}-blockers`;
  const blocked = blockers.length > 0;
  const held = blocked || busy;

  const act = (e: MouseEvent<HTMLButtonElement>) => {
    if (held) {
      e.preventDefault();
      return;
    }
    onSubmit?.();
  };

  return (
    <div className={className ? `ui-form-footer ${className}` : 'ui-form-footer'} data-pinned={pinned}>
      <div id={reasonsId} className="ui-form-footer-blockers" aria-live="polite">
        {blocked && (
          <>
            <span className="ui-visually-hidden">{t('ui.formFooter.blockers')}</span>
            <ul className="ui-form-footer-blocker-list">
              {blockers.map(reason => <li key={reason}>{reason}</li>)}
            </ul>
          </>
        )}
      </div>
      <div className="ui-form-footer-actions">
        {onCancel && <Button onClick={onCancel}>{cancelLabel ?? t('ui.formFooter.cancel')}</Button>}
        <Button
          variant={tone}
          type={onSubmit ? 'button' : 'submit'}
          aria-disabled={held || undefined}
          aria-busy={busy || undefined}
          aria-describedby={blocked ? reasonsId : undefined}
          onClick={act}
        >
          {submitLabel}
        </Button>
      </div>
    </div>
  );
}
