import { useState } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SearchableSelect, type SearchableSelectOption } from '../SearchableSelect';

const baseProps = {
  label: 'Model',
  placeholder: 'Search models…',
  emptyLabel: 'No matching model',
  clearLabel: 'Automatic',
};

describe('SearchableSelect', () => {
  it('filters a large model catalogue and selects the exact result', () => {
    const onChange = vi.fn();
    const options = Array.from({ length: 400 }, (_, index) => ({
      value: `provider/model-${index}`,
      label: `provider/model-${index}`,
    }));
    render(<SearchableSelect {...baseProps} value="" options={options} onChange={onChange} />);

    const input = screen.getByRole('combobox', { name: 'Model' });
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'model-396' } });

    expect(screen.getByRole('option', { name: 'provider/model-396' })).toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'provider/model-12' })).toBeNull();
    fireEvent.click(screen.getByRole('option', { name: 'provider/model-396' }));
    expect(onChange).toHaveBeenCalledWith('provider/model-396');
  });

  it('skips disabled options with the keyboard', () => {
    const onChange = vi.fn();
    const options: SearchableSelectOption[] = [
      { value: 'blocked', label: 'Blocked', disabled: true },
      { value: 'ready', label: 'Ready' },
    ];
    render(<SearchableSelect {...baseProps} value="" options={options} onChange={onChange} />);

    const input = screen.getByRole('combobox', { name: 'Model' });
    fireEvent.focus(input);
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'Enter' });

    expect(onChange).toHaveBeenCalledWith('ready');
  });

  it('clears a controlled selection without leaving stale search text', () => {
    function Harness() {
      const [value, setValue] = useState('selected');
      return (
        <SearchableSelect
          {...baseProps}
          value={value}
          options={[{ value: 'selected', label: 'Selected' }]}
          onChange={setValue}
        />
      );
    }
    render(<Harness />);

    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue('Selected');
    fireEvent.click(screen.getByRole('button', { name: 'Automatic' }));
    expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue('');
  });

  it('opens upward when there is not enough room below the control', () => {
    const rectSpy = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      top: 700,
      bottom: 735,
      left: 0,
      right: 300,
      width: 300,
      height: 35,
      x: 0,
      y: 700,
      toJSON: () => ({}),
    });
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: 800 });
    render(
      <SearchableSelect
        {...baseProps}
        value=""
        options={[{ value: 'model', label: 'Model' }]}
        onChange={vi.fn()}
      />,
    );

    const input = screen.getByRole('combobox', { name: 'Model' });
    fireEvent.focus(input);
    expect(input.closest('.searchable-select')).toHaveAttribute('data-placement', 'top');
    rectSpy.mockRestore();
  });

  it('offers no free-text entry unless allowCustomValue is set (KT-531)', () => {
    const onChange = vi.fn();
    render(
      <SearchableSelect
        {...baseProps}
        value=""
        options={[{ value: 'known-model', label: 'Known model' }]}
        onChange={onChange}
      />,
    );

    const input = screen.getByRole('combobox', { name: 'Model' });
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'unlisted-model' } });

    expect(screen.queryByRole('option', { name: 'unlisted-model' })).toBeNull();
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onChange).not.toHaveBeenCalled();
  });

  it('lets a typed id that matches nothing be confirmed via Enter (KT-531 unknown modality)', () => {
    const onChange = vi.fn();
    render(
      <SearchableSelect
        {...baseProps}
        value=""
        options={[{ value: 'known-model', label: 'Known model' }]}
        onChange={onChange}
        allowCustomValue
        customValueHint="Not verified"
      />,
    );

    const input = screen.getByRole('combobox', { name: 'Model' });
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'nvidia/unlisted-model' } });

    const custom = screen.getByRole('option', { name: 'nvidia/unlisted-model — Not verified' });
    expect(custom).toHaveAttribute('data-custom', 'true');
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onChange).toHaveBeenCalledWith('nvidia/unlisted-model');
  });

  it('lets a typed id that matches nothing be confirmed via click', () => {
    const onChange = vi.fn();
    render(
      <SearchableSelect
        {...baseProps}
        value=""
        options={[{ value: 'known-model', label: 'Known model' }]}
        onChange={onChange}
        allowCustomValue
      />,
    );

    const input = screen.getByRole('combobox', { name: 'Model' });
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'custom-id' } });
    fireEvent.click(screen.getByRole('option', { name: 'custom-id' }));

    expect(onChange).toHaveBeenCalledWith('custom-id');
  });

  it('does not duplicate an existing option as a free-text entry', () => {
    render(
      <SearchableSelect
        {...baseProps}
        value=""
        options={[{ value: 'known-model', label: 'Known model' }]}
        onChange={vi.fn()}
        allowCustomValue
      />,
    );

    const input = screen.getByRole('combobox', { name: 'Model' });
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'Known model' } });

    expect(screen.getAllByRole('option')).toHaveLength(1);
  });
});
