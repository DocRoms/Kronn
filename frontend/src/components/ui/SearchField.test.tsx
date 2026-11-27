// KT-1880 — SearchField: a labelled search input, a named clear button,
// Escape that clears without closing what is around it.
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { SearchField } from './SearchField';

function Field({ onOuterKey, showLabel }: { onOuterKey?: (key: string) => void; showLabel?: boolean }) {
  const [value, setValue] = useState('');
  return (
    <div onKeyDown={e => onOuterKey?.(e.key)}>
      <SearchField label="Search the tasks" placeholder="Title or id" value={value} onChange={setValue} showLabel={showLabel} />
    </div>
  );
}

describe('SearchField', () => {
  it('is a search box named by its label, hidden by default', () => {
    render(<Field />);
    const input = screen.getByRole('searchbox', { name: 'Search the tasks' });
    expect(input).toHaveAttribute('type', 'search');
    expect(input).toHaveAttribute('placeholder', 'Title or id');
    expect(screen.getByText('Search the tasks')).toHaveClass('ui-visually-hidden');
  });

  it('can show its label', () => {
    render(<Field showLabel />);
    expect(screen.getByText('Search the tasks')).toHaveClass('ui-search-field-label');
  });

  it('reports what is typed, and shows a named clear button only when there is text', async () => {
    const user = userEvent.setup();
    render(<Field />);
    expect(screen.queryByRole('button')).toBeNull();
    await user.type(screen.getByRole('searchbox'), 'KT-18');
    expect(screen.getByRole('searchbox')).toHaveValue('KT-18');
    await user.click(screen.getByRole('button', { name: 'ui.searchField.clear' }));
    expect(screen.getByRole('searchbox')).toHaveValue('');
    expect(screen.getByRole('searchbox')).toHaveFocus();
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('clears on Escape and keeps that Escape from the page; an empty field lets it through', async () => {
    const onOuterKey = vi.fn();
    const user = userEvent.setup();
    render(<Field onOuterKey={onOuterKey} />);
    await user.type(screen.getByRole('searchbox'), 'plan');
    onOuterKey.mockClear();
    await user.keyboard('{Escape}');
    expect(screen.getByRole('searchbox')).toHaveValue('');
    expect(onOuterKey).not.toHaveBeenCalled();
    await user.keyboard('{Escape}');
    expect(onOuterKey).toHaveBeenCalledWith('Escape');
  });

  it('calls the caller’s onKeyDown first, which can keep Escape for itself', () => {
    const onChange = vi.fn();
    render(<SearchField label="Search" value="x" onChange={onChange} onKeyDown={e => e.preventDefault()} />);
    fireEvent.keyDown(screen.getByRole('searchbox'), { key: 'Escape' });
    expect(onChange).not.toHaveBeenCalled();
  });

  it('takes its own id and a ref', () => {
    let node: HTMLInputElement | null = null;
    render(<SearchField id="task-search" label="Search" value="" onChange={vi.fn()} ref={el => { node = el; }} />);
    expect(screen.getByLabelText('Search')).toHaveAttribute('id', 'task-search');
    expect(node).toBe(screen.getByRole('searchbox'));
  });
});
