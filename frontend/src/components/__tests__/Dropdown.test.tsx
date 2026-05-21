import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, cleanup, within } from '@testing-library/react';
import { Dropdown } from '../Dropdown';

const OPTIONS = [
  { value: 'a', label: 'Alpha' },
  { value: 'b', label: 'Bravo', description: 'Second option' },
  { value: 'c', label: 'Charlie' },
];

describe('Dropdown', () => {
  beforeEach(() => cleanup());

  it('renders selected option label in trigger', () => {
    render(<Dropdown value="b" options={OPTIONS} onChange={() => {}} testId="d1" />);
    expect(screen.getByTestId('d1').textContent).toContain('Bravo');
  });

  it('opens list on click and shows all options', () => {
    render(<Dropdown value="a" options={OPTIONS} onChange={() => {}} testId="d2" />);
    fireEvent.click(screen.getByTestId('d2'));
    const listbox = screen.getByRole('listbox');
    const scoped = within(listbox);
    expect(scoped.getByText('Alpha')).toBeDefined();
    expect(scoped.getByText('Bravo')).toBeDefined();
    expect(scoped.getByText('Charlie')).toBeDefined();
  });

  it('invokes onChange with selected value and closes list', () => {
    const onChange = vi.fn();
    render(<Dropdown value="a" options={OPTIONS} onChange={onChange} testId="d3" />);
    fireEvent.click(screen.getByTestId('d3'));
    fireEvent.click(screen.getByTestId('d3-option-c'));
    expect(onChange).toHaveBeenCalledWith('c');
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('keyboard: ArrowDown opens then highlights next, Enter selects', () => {
    const onChange = vi.fn();
    render(<Dropdown value="a" options={OPTIONS} onChange={onChange} testId="d4" />);
    const trigger = screen.getByTestId('d4');
    trigger.focus();
    fireEvent.keyDown(trigger, { key: 'ArrowDown' });
    // List should be open now.
    expect(screen.getByRole('listbox')).toBeDefined();
    fireEvent.keyDown(trigger, { key: 'ArrowDown' });
    fireEvent.keyDown(trigger, { key: 'Enter' });
    expect(onChange).toHaveBeenCalled();
    // After 1 ArrowDown from value 'a' (index 0), highlight moves to 'b' (1).
    expect(onChange.mock.calls[0][0]).toBe('b');
  });

  it('keyboard: Escape closes without selecting', () => {
    const onChange = vi.fn();
    render(<Dropdown value="a" options={OPTIONS} onChange={onChange} testId="d5" />);
    fireEvent.click(screen.getByTestId('d5'));
    fireEvent.keyDown(screen.getByTestId('d5').parentElement!, { key: 'Escape' });
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('respects disabled options', () => {
    const onChange = vi.fn();
    const opts = [...OPTIONS, { value: 'd', label: 'Delta', disabled: true }];
    render(<Dropdown value="a" options={opts} onChange={onChange} testId="d6" />);
    fireEvent.click(screen.getByTestId('d6'));
    fireEvent.click(screen.getByTestId('d6-option-d'));
    expect(onChange).not.toHaveBeenCalled();
  });

  it('renders fallbackLabel when value is unknown', () => {
    render(
      <Dropdown
        value={'x' as 'a'}
        options={OPTIONS}
        onChange={() => {}}
        fallbackLabel="Custom — unsupported"
        testId="d7"
      />,
    );
    expect(screen.getByTestId('d7').textContent).toContain('Custom — unsupported');
  });

  it('disabled trigger does not open list', () => {
    render(<Dropdown value="a" options={OPTIONS} onChange={() => {}} disabled testId="d8" />);
    fireEvent.click(screen.getByTestId('d8'));
    expect(screen.queryByRole('listbox')).toBeNull();
  });
});
