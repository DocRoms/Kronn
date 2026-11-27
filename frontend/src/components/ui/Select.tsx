import type { ComponentPropsWithRef } from 'react';
import { ChevronDown } from 'lucide-react';
import './Select.css';

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export interface SelectGroup {
  label: string;
  options: readonly SelectOption[];
}

export type SelectEntry = SelectOption | SelectGroup;

export interface SelectProps
  extends Omit<ComponentPropsWithRef<'select'>, 'value' | 'defaultValue' | 'onChange' | 'size' | 'multiple' | 'children'> {
  value: string;
  onChange: (value: string) => void;
  /** Options in order; a group is an `optgroup`. */
  options: readonly SelectEntry[];
  /** The first, empty option shown while nothing is chosen. On a required
   *  select it cannot be chosen back. */
  placeholder?: string;
  size?: 'sm' | 'md';
}

const isGroup = (entry: SelectEntry): entry is SelectGroup => 'options' in entry;

function Option({ option }: { option: SelectOption }) {
  return <option value={option.value} disabled={option.disabled}>{option.label}</option>;
}

/** KT-1882 — the native select, styled once: the platform's own list, keyboard
 *  and screen-reader support. It has no label of its own: put it in a `Field`,
 *  which names it, or give it an `aria-label` where no label shows.
 *  `id`, `aria-*`, `disabled` and `ref` go to the `<select>`; `className` to
 *  its wrapper. */
export function Select({
  value, onChange, options, placeholder, size = 'md', className, ...rest
}: SelectProps) {
  const required = rest['aria-required'] === true || rest['aria-required'] === 'true' || rest.required === true;
  const empty = value === '';
  return (
    <div className={className ? `ui-select ${className}` : 'ui-select'} data-size={size}>
      <select
        {...rest}
        className="ui-select-input"
        value={value}
        data-empty={empty || undefined}
        onChange={e => onChange(e.target.value)}
      >
        {placeholder !== undefined && (
          <option value="" disabled={required && !empty}>{placeholder}</option>
        )}
        {options.map(entry => (isGroup(entry)
          ? (
            <optgroup key={`group:${entry.label}`} label={entry.label}>
              {entry.options.map(option => <Option key={option.value} option={option} />)}
            </optgroup>
          )
          : <Option key={entry.value} option={entry} />))}
      </select>
      <ChevronDown className="ui-select-chevron" aria-hidden="true" />
    </div>
  );
}
