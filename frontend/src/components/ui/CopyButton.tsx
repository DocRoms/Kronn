import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Check, CircleAlert, Copy } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { Button, type ButtonSize, type ButtonVariant } from './Button';
import { IconButton } from './IconButton';
import { COPY_FEEDBACK_MS, type CopyButtonForm } from './dataKinds';
import './visuallyHidden.css';
import './CopyButton.css';

type CopyState = 'idle' | 'copied' | 'failed';

interface CopyButtonBase {
  /** The action and its object ("Copy the task id"): the button's name, and
   *  its tooltip or its written label. Required. */
  label: string;
  form?: CopyButtonForm;
  size?: ButtonSize;
  /** The resting variant; a failed copy turns danger. Default ghost (icon)
   *  or neutral (text). */
  variant?: Exclude<ButtonVariant, 'danger'>;
  disabled?: boolean;
  className?: string;
}

/** The text to copy, known now, or read at the moment of the click. */
export type CopyButtonProps = CopyButtonBase & (
  | { text: string; getText?: never }
  | { getText: () => string | Promise<string>; text?: never }
);

const ICONS: Record<CopyState, ReactNode> = {
  idle: <Copy data-icon="copy" />,
  copied: <Check data-icon="check" />,
  failed: <CircleAlert data-icon="failed" />,
};

/** KT-1886 — copies a text to the clipboard and says so: the icon turns into
 *  a check for about 1.5 s and a polite live region says "Copied"; when the
 *  clipboard refuses (no permission, no secure context, no clipboard), the
 *  button turns danger and the region says "Copy failed". The name never
 *  changes, so focus does not re-announce it. */
export function CopyButton(props: CopyButtonProps) {
  const { label, form = 'icon', size = 'md', variant, disabled, className } = props;
  const { t } = useT();
  const [state, setState] = useState<CopyState>('idle');
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  const show = (next: CopyState) => {
    if (!mounted.current) return;
    setState(next);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      timer.current = null;
      if (mounted.current) setState('idle');
    }, COPY_FEEDBACK_MS);
  };

  const copy = async () => {
    try {
      const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : undefined;
      if (!clipboard?.writeText) throw new Error('no clipboard');
      const value = props.getText ? await props.getText() : props.text;
      await clipboard.writeText(value ?? '');
      show('copied');
    } catch {
      show('failed');
    }
  };

  const shownVariant: ButtonVariant = state === 'failed' ? 'danger' : variant ?? (form === 'icon' ? 'ghost' : 'neutral');
  const classes = ['ui-copy-button', className].filter(Boolean).join(' ');
  const common = {
    className: classes,
    size,
    disabled,
    variant: shownVariant,
    'data-state': state,
    onClick: () => { void copy(); },
  };
  const message = state === 'copied' ? t('ui.copy.copied') : state === 'failed' ? t('ui.copy.failed') : '';

  return (
    <span className="ui-copy-button-wrap">
      {form === 'icon'
        ? <IconButton {...common} label={label} icon={ICONS[state]} />
        : <Button {...common} icon={ICONS[state]}>{label}</Button>}
      <span className="ui-copy-button-live ui-visually-hidden" role="status" aria-live="polite">{message}</span>
    </span>
  );
}
