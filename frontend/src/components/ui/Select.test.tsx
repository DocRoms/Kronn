// KT-1882 — Select: the native select, styled once, with an optional
// placeholder, groups, and the value given back as a string.
import { describe, expect, it, vi } from 'vitest';
import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Select, type SelectEntry } from './Select';
import { describeViolations, runAxe } from '../../test/axe';

const HERE = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(join(HERE, 'Select.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

const AGENTS: SelectEntry[] = [
  { value: 'claude', label: 'Claude Code' },
  { value: 'codex', label: 'Codex' },
  { value: 'kiro', label: 'Kiro', disabled: true },
];

function Harness({ placeholder, onValue }: { placeholder?: string; onValue?: (v: string) => void }) {
  const [value, setValue] = useState('');
  return (
    <Select
      aria-label="Agent"
      value={value}
      placeholder={placeholder}
      onChange={v => { setValue(v); onValue?.(v); }}
      options={AGENTS}
    />
  );
}

describe('Select', () => {
  it('is a native combobox showing its options in order, a disabled one disabled', () => {
    render(<Select aria-label="Agent" value="codex" onChange={() => {}} options={AGENTS} />);
    const select = screen.getByRole('combobox', { name: 'Agent' });
    expect(select.tagName).toBe('SELECT');
    expect(select).toHaveValue('codex');
    const options = within(select).getAllByRole('option');
    expect(options.map(o => o.textContent)).toEqual(['Claude Code', 'Codex', 'Kiro']);
    expect(options[2]).toBeDisabled();
  });

  it('gives back the chosen value as a string', async () => {
    const onValue = vi.fn();
    const user = userEvent.setup();
    render(<Harness placeholder="Choose an agent" onValue={onValue} />);
    await user.selectOptions(screen.getByRole('combobox', { name: 'Agent' }), 'codex');
    expect(onValue).toHaveBeenCalledWith('codex');
    expect(screen.getByRole('combobox')).toHaveValue('codex');
  });

  it('shows its placeholder as the first, empty option while nothing is chosen', () => {
    render(<Harness placeholder="Choose an agent" />);
    const select = screen.getByRole('combobox');
    const first = within(select).getAllByRole('option')[0] as HTMLOptionElement;
    expect(first).toHaveTextContent('Choose an agent');
    expect(first.value).toBe('');
    expect(select).toHaveValue('');
    expect(select).toHaveAttribute('data-empty', 'true');
  });

  it('cannot go back to the placeholder once a required select has a value', () => {
    render(<Select aria-label="Agent" aria-required value="claude" placeholder="Choose" onChange={() => {}} options={AGENTS} />);
    const first = within(screen.getByRole('combobox')).getAllByRole('option')[0];
    expect(first).toBeDisabled();
    expect(screen.getByRole('combobox')).not.toHaveAttribute('data-empty');
  });

  it('renders groups of options', () => {
    render(
      <Select
        aria-label="Model"
        value="sonnet"
        onChange={() => {}}
        options={[
          { label: 'Anthropic', options: [{ value: 'sonnet', label: 'Sonnet' }, { value: 'opus', label: 'Opus' }] },
          { label: 'Local', options: [{ value: 'qwen', label: 'Qwen' }] },
        ]}
      />,
    );
    const groups = screen.getAllByRole('group');
    expect(groups.map(g => g.getAttribute('label'))).toEqual(['Anthropic', 'Local']);
    expect(within(groups[0]).getAllByRole('option')).toHaveLength(2);
  });

  it('passes its id, its description, its state and a ref to the native select', () => {
    let node: HTMLSelectElement | null = null;
    render(
      <>
        <p id="note">Note</p>
        <Select
          id="agent-select"
          aria-label="Agent"
          aria-describedby="note"
          aria-invalid
          disabled
          size="sm"
          className="extra"
          ref={el => { node = el; }}
          value="claude"
          onChange={() => {}}
          options={AGENTS}
        />
      </>,
    );
    const select = screen.getByRole('combobox', { name: 'Agent' });
    expect(select).toHaveAttribute('id', 'agent-select');
    expect(select).toHaveAccessibleDescription('Note');
    expect(select).toHaveAttribute('aria-invalid', 'true');
    expect(select).toBeDisabled();
    expect(node).toBe(select);
    const wrapper = select.closest('.ui-select')!;
    expect(wrapper).toHaveAttribute('data-size', 'sm');
    expect(wrapper).toHaveClass('extra');
    expect(wrapper.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
  });

  it('passes axe', async () => {
    render(<main><Harness placeholder="Choose an agent" /></main>);
    const results = await act(() => runAxe(document.body));
    expect(describeViolations(results)).toEqual([]);
  });

  it('writes 16px text on a coarse pointer, so a phone does not zoom on focus', () => {
    const coarse = CSS.slice(CSS.indexOf('@media (pointer: coarse)'));
    expect(coarse).toMatch(/\.ui-select-input[^{]*\{[^}]*font-size:\s*var\(--kr-type-xl\)/);
  });
});
