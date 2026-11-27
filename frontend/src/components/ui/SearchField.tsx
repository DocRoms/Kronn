import { useId, useRef, type ComponentPropsWithRef, type KeyboardEvent } from 'react';
import { Search, X } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { IconButton } from './IconButton';
import './SearchField.css';

export interface SearchFieldProps
  extends Omit<ComponentPropsWithRef<'input'>, 'type' | 'value' | 'defaultValue' | 'onChange' | 'size' | 'aria-label'> {
  /** The field's label, required. Hidden unless `showLabel`, but always its accessible name. */
  label: string;
  /** Not accepted: `label` names the field (declared so JSX checks it). */
  'aria-label'?: never;
  value: string;
  onChange: (value: string) => void;
  showLabel?: boolean;
  size?: 'sm' | 'md';
}

/** KT-1880 — a search input with its label, a search icon and a clear button.
 *  Escape clears a non-empty field and goes no further (a dialog around it
 *  stays open); on an empty field Escape is left to the page. */
export function SearchField({
  label, value, onChange, showLabel = false, size = 'md', className, id, onKeyDown, ref, ...rest
}: SearchFieldProps) {
  const { t } = useT();
  const ownId = useId();
  const inputId = id ?? ownId;
  const inputRef = useRef<HTMLInputElement | null>(null);

  const setRef = (el: HTMLInputElement | null) => {
    inputRef.current = el;
    if (typeof ref === 'function') ref(el);
    else if (ref) ref.current = el;
  };

  const clear = () => {
    onChange('');
    inputRef.current?.focus();
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    onKeyDown?.(e);
    if (e.defaultPrevented) return;
    if (e.key === 'Escape' && value !== '') {
      e.preventDefault();
      e.stopPropagation();
      onChange('');
    }
  };

  return (
    <div className={className ? `ui-search-field ${className}` : 'ui-search-field'} data-size={size}>
      <label htmlFor={inputId} className={showLabel ? 'ui-search-field-label' : 'ui-visually-hidden'}>
        {label}
      </label>
      <div className="ui-search-field-control">
        <Search className="ui-search-field-icon" aria-hidden="true" />
        <input
          {...rest}
          ref={setRef}
          id={inputId}
          type="search"
          className="ui-search-field-input"
          value={value}
          onChange={e => onChange(e.target.value)}
          onKeyDown={handleKeyDown}
        />
        {value !== '' && (
          <IconButton
            className="ui-search-field-clear"
            label={t('ui.searchField.clear')}
            icon={<X />}
            size="sm"
            tooltip={false}
            onClick={clear}
          />
        )}
      </div>
    </div>
  );
}
