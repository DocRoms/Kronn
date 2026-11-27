import { useId, type ComponentPropsWithRef, type ReactNode } from 'react';
import './visuallyHidden.css';
import './Switch.css';

export interface SwitchProps
  extends Omit<ComponentPropsWithRef<'input'>,
    'type' | 'role' | 'checked' | 'defaultChecked' | 'value' | 'onChange' | 'size' | 'aria-label' | 'aria-checked'> {
  /** The visible label, which is also the switch's accessible name. */
  label: ReactNode;
  /** Not accepted: `label` names the switch (declared so JSX checks it). */
  'aria-label'?: never;
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** A line under the label: what turning it on does. */
  description?: ReactNode;
  /** Keep the label for assistive technology only, where the row around the
   *  switch already shows it. */
  hideLabel?: boolean;
}

/** KT-1882 — an on/off setting that applies at once. A native checkbox with
 *  the `switch` role: Space toggles it, its label is a real `<label>` (a click
 *  on it toggles too), and a form reads it like any checkbox. */
export function Switch({
  label, checked, onChange, description, hideLabel = false, disabled = false, id, className,
  'aria-describedby': describedBy, ...rest
}: SwitchProps) {
  const ownId = useId();
  const inputId = id ?? ownId;
  const descriptionId = `${ownId}-description`;
  const hasDescription = description !== undefined && description !== null && description !== false && description !== '';
  return (
    <div className={className ? `ui-switch ${className}` : 'ui-switch'} data-disabled={disabled || undefined}>
      <label className="ui-switch-row" htmlFor={inputId}>
        <input
          {...rest}
          id={inputId}
          type="checkbox"
          role="switch"
          className="ui-switch-input"
          checked={checked}
          disabled={disabled}
          aria-describedby={[hasDescription ? descriptionId : '', describedBy ?? ''].filter(Boolean).join(' ') || undefined}
          onChange={e => onChange(e.target.checked)}
        />
        <span className="ui-switch-track" aria-hidden="true">
          <span className="ui-switch-thumb" />
        </span>
        <span className={hideLabel ? 'ui-visually-hidden' : 'ui-switch-label'}>{label}</span>
      </label>
      {hasDescription && <p id={descriptionId} className="ui-switch-description">{description}</p>}
    </div>
  );
}
