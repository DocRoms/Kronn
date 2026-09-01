import { ArrowDown, ArrowUp, Plus, Trash2 } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import type { PromptVariable, PromptVariableControl, PromptVariableOption } from '../../types/generated';

interface Props {
  variable: PromptVariable;
  onChange: (variable: PromptVariable) => void;
}

const controlType = (variable: PromptVariable): PromptVariableControl['type'] => variable.control?.type ?? 'text';

export function PromptVariableControlEditor({ variable, onChange }: Props) {
  const { t } = useT();
  const type = controlType(variable);
  const select = variable.control?.type === 'select' ? variable.control : null;
  const options = select?.options ?? [];

  const setOptions = (next: PromptVariableOption[], defaultValue = select?.default_value) => {
    onChange({
      ...variable,
      control: {
        type: 'select',
        options: next,
        ...(defaultValue ? { default_value: defaultValue } : {}),
      },
    });
  };

  const setType = (next: PromptVariableControl['type']) => {
    if (next === 'text') onChange({ ...variable, control: undefined });
    else if (next === 'textarea') onChange({ ...variable, control: { type: 'textarea' } });
    else onChange({
      ...variable,
      control: select ?? {
        type: 'select',
        options: [{ value: 'option_1', label: t('variables.optionDefault', 1), enabled: true }],
      },
    });
  };

  const addOption = () => {
    let number = options.length + 1;
    while (options.some(option => option.value === `option_${number}`)) number += 1;
    setOptions([...options, {
      value: `option_${number}`,
      label: t('variables.optionDefault', number),
      enabled: true,
    }]);
  };

  const updateOption = (index: number, patch: Partial<PromptVariableOption>) => {
    const next = options.map((option, optionIndex) => optionIndex === index ? { ...option, ...patch } : option);
    const changed = next[index];
    const defaultValue = changed && !changed.enabled && select?.default_value === changed.value
      ? undefined
      : select?.default_value;
    setOptions(next, defaultValue);
  };

  const moveOption = (index: number, direction: -1 | 1) => {
    const destination = index + direction;
    if (destination < 0 || destination >= options.length) return;
    const next = [...options];
    [next[index], next[destination]] = [next[destination], next[index]];
    setOptions(next);
  };

  return (
    <div className="prompt-variable-control-editor">
      <label className="prompt-variable-control-label">
        <span>{t('variables.controlType')}</span>
        <select
          className="wf-select text-xs"
          value={type}
          onChange={event => setType(event.target.value as PromptVariableControl['type'])}
          aria-label={t('variables.controlTypeFor', variable.label || variable.name)}
        >
          <option value="text">{t('variables.controlText')}</option>
          <option value="textarea">{t('variables.controlTextarea')}</option>
          <option value="select">{t('variables.controlSelect')}</option>
        </select>
      </label>

      {select && (
        <div className="prompt-variable-options">
          <div className="prompt-variable-options-header">
            <span>{t('variables.options')}</span>
            <button type="button" className="wf-small-btn" onClick={addOption}>
              <Plus size={11} /> {t('variables.addOption')}
            </button>
          </div>
          {options.map((option, index) => {
            const activeCount = options.filter(candidate => candidate.enabled).length;
            return (
              <div className="prompt-variable-option" key={`${index}-${option.value}`}>
                <input
                  className="wf-input"
                  value={option.label}
                  onChange={event => updateOption(index, { label: event.target.value })}
                  placeholder={t('variables.optionLabel')}
                  aria-label={t('variables.optionLabelAt', index + 1)}
                />
                <input
                  className="wf-input prompt-variable-option-value"
                  value={option.value}
                  onChange={event => updateOption(index, { value: event.target.value })}
                  placeholder={t('variables.optionValue')}
                  aria-label={t('variables.optionValueAt', index + 1)}
                />
                <label className="prompt-variable-option-enabled">
                  <input
                    type="checkbox"
                    checked={option.enabled}
                    disabled={option.enabled && activeCount === 1}
                    onChange={event => updateOption(index, { enabled: event.target.checked })}
                  />
                  {t('variables.optionActive')}
                </label>
                <button type="button" className="wf-icon-btn" disabled={index === 0} onClick={() => moveOption(index, -1)} aria-label={t('variables.optionMoveUp')}><ArrowUp size={11} /></button>
                <button type="button" className="wf-icon-btn" disabled={index === options.length - 1} onClick={() => moveOption(index, 1)} aria-label={t('variables.optionMoveDown')}><ArrowDown size={11} /></button>
                <button type="button" className="wf-icon-btn" disabled={options.length === 1} onClick={() => {
                  const next = options.filter((_, optionIndex) => optionIndex !== index);
                  setOptions(next, select.default_value === option.value ? undefined : select.default_value);
                }} aria-label={t('variables.optionRemove')}><Trash2 size={11} /></button>
              </div>
            );
          })}
          <label className="prompt-variable-default">
            <span>{t('variables.defaultValue')}</span>
            <select
              className="wf-select text-xs"
              value={select.default_value ?? ''}
              onChange={event => setOptions(options, event.target.value || undefined)}
            >
              <option value="">{t('variables.noDefault')}</option>
              {options.filter(option => option.enabled).map(option => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </select>
          </label>
        </div>
      )}
    </div>
  );
}
