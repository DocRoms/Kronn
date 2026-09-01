import { SearchableSelect } from '../SearchableSelect';
import { useT } from '../../lib/I18nContext';
import type { PromptVariable } from '../../types/generated';
import { promptVariableEffectiveValue } from '../../lib/promptVariableControl';

interface Props {
  variable: PromptVariable;
  value?: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  autoFocus?: boolean;
  onEnter?: () => void;
}

export function PromptVariableInput({
  variable,
  value,
  onChange,
  disabled = false,
  autoFocus = false,
  onEnter,
}: Props) {
  const { t } = useT();
  const label = variable.label || variable.name;
  const control = variable.control;
  const effectiveValue = promptVariableEffectiveValue(variable, value);

  if (control?.type === 'textarea') {
    return (
      <textarea
        className="wf-textarea flex-1 prompt-variable-textarea"
        rows={4}
        value={effectiveValue}
        onChange={event => onChange(event.target.value)}
        placeholder={variable.placeholder}
        aria-label={label}
        disabled={disabled}
        autoFocus={autoFocus}
      />
    );
  }

  if (control?.type === 'select') {
    const activeOptions = control.options.filter(option => option.enabled);
    if (activeOptions.length > 8) {
      return (
        <SearchableSelect
          className="searchable-select--compact prompt-variable-select"
          value={effectiveValue}
          options={activeOptions.map(option => ({ value: option.value, label: option.label }))}
          onChange={onChange}
          label={label}
          placeholder={variable.placeholder || t('variables.selectPlaceholder')}
          emptyLabel={t('variables.selectEmpty')}
          clearLabel={variable.required ? undefined : t('variables.selectNone')}
          clearable={!variable.required}
          disabled={disabled}
        />
      );
    }
    return (
      <select
        className="wf-select flex-1 prompt-variable-select"
        value={effectiveValue}
        onChange={event => onChange(event.target.value)}
        aria-label={label}
        disabled={disabled}
        autoFocus={autoFocus}
      >
        {!variable.required && <option value="">{t('variables.selectNone')}</option>}
        {variable.required && !effectiveValue && (
          <option value="" disabled>{variable.placeholder || t('variables.selectPlaceholder')}</option>
        )}
        {activeOptions.map(option => (
          <option key={option.value} value={option.value}>{option.label}</option>
        ))}
      </select>
    );
  }

  return (
    <input
      className="wf-input flex-1"
      value={effectiveValue}
      onChange={event => onChange(event.target.value)}
      placeholder={variable.placeholder}
      aria-label={label}
      disabled={disabled}
      autoFocus={autoFocus}
      onKeyDown={event => {
        if (event.key === 'Enter' && onEnter) {
          event.preventDefault();
          onEnter();
        }
      }}
    />
  );
}
