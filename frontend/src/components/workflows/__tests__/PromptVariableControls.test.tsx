import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import { I18nProvider } from '../../../lib/I18nContext';
import type { PromptVariable } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock());

import { PromptVariableInput } from '../PromptVariableInput';
import { PromptVariableControlEditor } from '../PromptVariableControlEditor';

const baseVariable = (partial: Partial<PromptVariable> = {}): PromptVariable => ({
  name: 'language',
  label: 'Language',
  placeholder: 'Choose',
  description: null,
  required: true,
  source: 'user_input',
  source_ref: null,
  allow_manual_override: false,
  ...partial,
});

const wrap = (ui: React.ReactNode) => render(<I18nProvider>{ui}</I18nProvider>);

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('PromptVariableInput', () => {
  it('keeps legacy variables as single-line text inputs', () => {
    const onChange = vi.fn();
    wrap(<PromptVariableInput variable={baseVariable()} value="fr" onChange={onChange} />);
    const input = screen.getByRole('textbox', { name: 'Language' });
    expect(input.tagName).toBe('INPUT');
    fireEvent.change(input, { target: { value: 'en' } });
    expect(onChange).toHaveBeenCalledWith('en');
  });

  it('renders multiline variables as textareas without treating Enter as submit', () => {
    const onChange = vi.fn();
    const onEnter = vi.fn();
    wrap(<PromptVariableInput
      variable={baseVariable({ control: { type: 'textarea' } })}
      value="first"
      onChange={onChange}
      onEnter={onEnter}
    />);
    const input = screen.getByRole('textbox', { name: 'Language' });
    expect(input.tagName).toBe('TEXTAREA');
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onEnter).not.toHaveBeenCalled();
  });

  it('uses the active select options and its declared default', () => {
    const onChange = vi.fn();
    wrap(<PromptVariableInput
      variable={baseVariable({
        control: {
          type: 'select',
          options: [
            { value: 'fr', label: 'Français', enabled: true },
            { value: 'en', label: 'English', enabled: false },
          ],
          default_value: 'fr',
        },
      })}
      onChange={onChange}
    />);
    const select = screen.getByRole('combobox', { name: 'Language' });
    expect(select).toHaveValue('fr');
    expect(screen.getByRole('option', { name: 'Français' })).toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'English' })).not.toBeInTheDocument();
  });

  it('switches to the searchable selector for a long option list', () => {
    const options = Array.from({ length: 9 }, (_, index) => ({
      value: `value-${index}`,
      label: `Choice ${index}`,
      enabled: true,
    }));
    wrap(<PromptVariableInput
      variable={baseVariable({ control: { type: 'select', options } })}
      onChange={vi.fn()}
    />);
    const input = screen.getByRole('combobox', { name: 'Language' });
    expect(input).toHaveAttribute('type', 'search');
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'Choice 8' } });
    expect(screen.getByRole('option', { name: 'Choice 8' })).toBeInTheDocument();
  });
});

function EditorHarness() {
  const [variable, setVariable] = useState(baseVariable());
  return <PromptVariableControlEditor variable={variable} onChange={setVariable} />;
}

describe('PromptVariableControlEditor', () => {
  it('configures a versioned single-select with stable values and defaults', () => {
    wrap(<EditorHarness />);
    const type = screen.getByRole('combobox', { name: /variables\.controlTypeFor|Type de champ|Field type/i });
    fireEvent.change(type, { target: { value: 'select' } });

    fireEvent.click(screen.getByRole('button', { name: /variables\.addOption|Ajouter une option|Add option/i }));
    const labels = screen.getAllByRole('textbox', { name: /variables\.optionLabelAt|Libellé|Label/i });
    const values = screen.getAllByRole('textbox', { name: /variables\.optionValueAt|Valeur|Value/i });
    fireEvent.change(labels[1], { target: { value: 'English' } });
    fireEvent.change(values[1], { target: { value: 'en' } });

    const defaults = screen.getAllByRole('combobox');
    fireEvent.change(defaults[defaults.length - 1], { target: { value: 'en' } });
    expect(defaults[defaults.length - 1]).toHaveValue('en');
  });
});
