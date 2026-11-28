import type { ComponentPropsWithRef, ReactNode } from 'react';
import type { ButtonSize, ButtonVariant } from './Button';
import { Tooltip } from './Tooltip';
import './Button.css';
import './IconButton.css';

/** A string type that refuses the empty literal: `label=""` does not compile. */
export type NonEmptyLabel<L extends string> = L extends '' ? never : L;

/** The native props an IconButton takes. Its name comes from `label` only:
 *  `aria-label`, `aria-labelledby`, `title` and children are not accepted, so
 *  there is no second, diverging way to name it. (JSX does not check a
 *  hyphenated attribute against the props unless the prop is declared: the
 *  two aria ones are declared `never`.) */
export type IconButtonNativeProps = Omit<
  ComponentPropsWithRef<'button'>,
  'children' | 'aria-label' | 'aria-labelledby' | 'title'
> & { 'aria-label'?: never; 'aria-labelledby'?: never };

export interface IconButtonProps<L extends string = string> extends IconButtonNativeProps {
  /** The accessible name, required: an IconButton without one does not compile.
   *  It is also the tooltip shown on hover and keyboard focus. */
  label: NonEmptyLabel<L>;
  /** The icon, hidden from assistive technology: `label` names the button. */
  icon: ReactNode;
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Show `label` as a tooltip on hover and focus (default true). */
  tooltip?: boolean;
}

/** KT-1880 — a button that shows only an icon. Square, at the control height
 *  of its size; at least --kr-control-h-touch on a coarse pointer. */
export function IconButton<L extends string>({
  label, icon, variant = 'ghost', size = 'md', tooltip = true, className, type = 'button', ...rest
}: IconButtonProps<L>) {
  const button = (
    <button
      {...rest}
      type={type}
      aria-label={label}
      className={className ? `ui-icon-button ${className}` : 'ui-icon-button'}
      data-variant={variant}
      data-size={size}
    >
      <span className="ui-button-icon" aria-hidden="true">{icon}</span>
    </button>
  );
  // The tooltip repeats the name: it is not wired as a description too.
  return tooltip ? <Tooltip content={label} describe={false}>{button}</Tooltip> : button;
}
