import type { ComponentPropsWithRef, ReactNode } from 'react';
import './Button.css';

/** primary: the one main action of a view; neutral: any other action;
 *  ghost: a low-emphasis action in a dense row; danger: a destructive action. */
export type ButtonVariant = 'primary' | 'neutral' | 'ghost' | 'danger';
/** sm = --kr-control-h-sm (dense rows), md = --kr-control-h-md (default),
 *  lg = --kr-control-h-lg. On a coarse pointer every size is at least
 *  --kr-control-h-touch high. */
export type ButtonSize = 'sm' | 'md' | 'lg';

export interface ButtonProps extends ComponentPropsWithRef<'button'> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** An icon before the label, hidden from assistive technology. */
  icon?: ReactNode;
  /** The visible label, which is also the accessible name. */
  children: ReactNode;
}

/** KT-1880 — the base button. `type` is "button" unless said otherwise, so a
 *  button inside a form never submits it by accident. */
export function Button({
  variant = 'neutral', size = 'md', icon, children, className, type = 'button', ...rest
}: ButtonProps) {
  return (
    <button
      {...rest}
      type={type}
      className={className ? `ui-button ${className}` : 'ui-button'}
      data-variant={variant}
      data-size={size}
    >
      {icon && <span className="ui-button-icon" aria-hidden="true">{icon}</span>}
      <span className="ui-button-label">{children}</span>
    </button>
  );
}
