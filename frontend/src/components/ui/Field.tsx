import { cloneElement, isValidElement, useId, type ReactElement, type ReactNode } from 'react';
import { AlertCircle } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import './Field.css';

/** What Field gives its control: its id (the label's `for`), its description
 *  (the error first, then the help), its invalid and required states. */
export interface FieldControlProps {
  id: string;
  'aria-describedby'?: string;
  'aria-invalid'?: true;
  'aria-required'?: true;
  disabled?: boolean;
}

interface FieldBaseProps {
  /** The visible label, above the control, always bound to it. */
  label: ReactNode;
  /** One control element (cloned with `FieldControlProps`), or a function that
   *  places them on a control Field cannot reach. */
  children: ReactElement | ((control: FieldControlProps) => ReactNode);
  /** A line under the control: what to write, in what format. */
  help?: ReactNode;
  /** What is wrong, under the control. While set, the control is invalid and
   *  the line is announced. */
  error?: ReactNode;
  disabled?: boolean;
  className?: string;
}

/** A field is required (an asterisk), optional (the word "optional"), or
 *  neither: never both. */
export type FieldProps = FieldBaseProps & (
  | { required?: boolean; optional?: never }
  | { optional?: boolean; required?: never }
);

/** Native elements that take the field style; a component keeps its own. */
const NATIVE_TEXT_CONTROLS = new Set(['input', 'textarea', 'select']);

/** The asterisk means "required" and nothing else: one at the end of a text
 *  label is dropped (KT-1882). */
function plainLabel(label: ReactNode): ReactNode {
  return typeof label === 'string' ? label.replace(/\s*\*+\s*$/, '') : label;
}

const join = (...ids: (string | undefined | false)[]) => ids.filter(Boolean).join(' ') || undefined;

/** KT-1882 — a form field: its label above the control and bound to it, an
 *  optional marker, a help line and an error line under the control. The
 *  error line sits in a polite live region rendered from the start, so it is
 *  announced when it appears. */
export function Field({
  label, children, help, error, required = false, optional = false, disabled = false, className,
}: FieldProps) {
  const { t } = useT();
  const ownId = useId();
  const child = typeof children === 'function' ? null : children;
  const childProps = (isValidElement(child) ? child.props : {}) as {
    id?: string; className?: string; 'aria-describedby'?: string; disabled?: boolean;
  };
  const controlId = childProps.id ?? `${ownId}-control`;
  const helpId = `${ownId}-help`;
  const errorId = `${ownId}-error`;
  const hasError = error !== undefined && error !== null && error !== false && error !== '';
  const hasHelp = help !== undefined && help !== null && help !== false && help !== '';

  const control: FieldControlProps = {
    id: controlId,
    'aria-describedby': join(hasError && errorId, hasHelp && helpId, childProps['aria-describedby']),
    ...(hasError ? { 'aria-invalid': true as const } : {}),
    ...(required ? { 'aria-required': true as const } : {}),
    ...(disabled ? { disabled: true } : {}),
  };

  let rendered: ReactNode;
  if (typeof children === 'function') {
    rendered = children(control);
  } else {
    const native = typeof children.type === 'string' && NATIVE_TEXT_CONTROLS.has(children.type);
    rendered = cloneElement(children, {
      ...control,
      ...(native ? { className: join('ui-field-control', childProps.className) } : {}),
    } as Partial<FieldControlProps>);
  }

  return (
    <div
      className={className ? `ui-field ${className}` : 'ui-field'}
      data-invalid={hasError || undefined}
      data-disabled={disabled || undefined}
    >
      <label className="ui-field-label" htmlFor={controlId}>
        {plainLabel(label)}
        {required && <span className="ui-field-required" aria-hidden="true">*</span>}
        {optional && <span className="ui-field-optional">{t('ui.field.optional')}</span>}
      </label>
      {rendered}
      {hasHelp && <p id={helpId} className="ui-field-help">{help}</p>}
      <div className="ui-field-error-slot" aria-live="polite">
        {hasError && (
          <p id={errorId} className="ui-field-error">
            <AlertCircle className="ui-field-error-icon" aria-hidden="true" />
            <span>{error}</span>
          </p>
        )}
      </div>
    </div>
  );
}
