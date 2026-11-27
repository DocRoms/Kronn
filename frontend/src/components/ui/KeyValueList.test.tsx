// KT-1886 — KeyValueList: a description list of labels and values, in a grid
// or one row per item; a missing value shows "—"; mono values wrap anywhere;
// a long value can be cut with its full text in a title; a value can carry a
// CopyButton named after its label; in a narrow container the label goes
// above its value. Written before the component.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { KeyValueList, type KeyValueItem } from './KeyValueList';
import { cssText } from '../../test/cssBlocks';
import { describeViolations, runAxe } from '../../test/axe';

const CSS = cssText(join(dirname(fileURLToPath(import.meta.url)), 'KeyValueList.css'));

const ITEMS: KeyValueItem[] = [
  { label: 'Branch', value: 'kronn/task/kt-1886', mono: true, copyable: true },
  { label: 'Agent', value: 'Claude Code' },
  { label: 'Started', value: null },
  { label: 'Worktree', value: '/Users/someone/very/long/path/that/goes/on', truncate: true },
];

const writeText = vi.fn<(text: string) => Promise<void>>();
const original = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
beforeEach(() => {
  writeText.mockReset();
  writeText.mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
});
afterEach(() => {
  if (original) Object.defineProperty(navigator, 'clipboard', original);
  else delete (navigator as { clipboard?: unknown }).clipboard;
});

describe('KeyValueList', () => {
  it('is a dl: each label a dt, each value its dd, in order', () => {
    const { container } = render(<KeyValueList items={ITEMS} />);
    const dl = container.querySelector('dl')!;
    expect(dl).toHaveClass('ui-kv');
    expect([...dl.querySelectorAll('dt')].map(dt => dt.textContent)).toEqual(ITEMS.map(i => i.label));
    expect(dl.querySelectorAll('dd')).toHaveLength(ITEMS.length);
    // A dt and its dd sit together in a div, which a dl allows.
    for (const group of dl.children) {
      expect(group.tagName).toBe('DIV');
      expect([...group.children].map(c => c.tagName)).toEqual(['DT', 'DD']);
    }
  });

  it('lays out as a grid (labels in a max-content column) by default, or one row per item', () => {
    const { container, rerender } = render(<KeyValueList items={ITEMS} />);
    expect(container.querySelector('dl')).toHaveAttribute('data-layout', 'grid');
    expect(CSS).toMatch(/\.ui-kv\[data-layout="grid"\]\s*\{[^}]*grid-template-columns:\s*max-content\s+minmax\(0,\s*1fr\)/);
    rerender(<KeyValueList items={ITEMS} layout="inline" />);
    expect(container.querySelector('dl')).toHaveAttribute('data-layout', 'inline');
    expect(CSS).toMatch(/\.ui-kv\[data-layout="inline"\] \.ui-kv-item\s*\{[^}]*justify-content:\s*space-between/);
    expect(CSS).toMatch(/\.ui-kv\[data-layout="inline"\] dd\s*\{[^}]*font-variant-numeric:\s*tabular-nums/);
  });

  it('shows "—" for a missing value, and says "No value" to assistive technology', () => {
    render(<KeyValueList items={[{ label: 'Started', value: null }, { label: 'Ended' }, { label: 'Note', value: '' }]} />);
    for (const dd of document.querySelectorAll('dd')) {
      expect(dd).toHaveAttribute('data-missing', 'true');
      expect(dd.querySelector('[aria-hidden="true"]')).toHaveTextContent('—');
      expect(dd.querySelector('.ui-visually-hidden')).toHaveTextContent('ui.keyValue.missing');
    }
    // Nothing to copy.
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('keeps 0 and false-like text as values, not as missing', () => {
    render(<KeyValueList items={[{ label: 'Retries', value: 0 }]} />);
    expect(document.querySelector('dd')).not.toHaveAttribute('data-missing');
    expect(document.querySelector('dd')).toHaveTextContent('0');
  });

  it('writes a mono value in the mono font, wrapping anywhere', () => {
    render(<KeyValueList items={ITEMS} />);
    const value = screen.getByText('kronn/task/kt-1886');
    expect(value.closest('dd')).toHaveAttribute('data-mono', 'true');
    expect(CSS).toMatch(/dd\[data-mono="true"\][^{]*\{[^}]*font-family:\s*var\(--kr-font-mono\)/);
    expect(CSS).toMatch(/dd\[data-mono="true"\][^{]*\{[^}]*overflow-wrap:\s*anywhere/);
  });

  it('cuts a truncate value with an ellipsis, its full value in a title', () => {
    render(<KeyValueList items={ITEMS} />);
    const value = screen.getByText(ITEMS[3].value as string);
    expect(value).toHaveClass('ui-kv-truncate');
    expect(value).toHaveAttribute('title', ITEMS[3].value);
    expect(CSS).toMatch(/\.ui-kv-truncate\s*\{[^}]*text-overflow:\s*ellipsis/);
  });

  it('adds a CopyButton named after the label to a copyable value, which copies the value', async () => {
    render(<KeyValueList items={ITEMS} />);
    const dd = screen.getByText('kronn/task/kt-1886').closest('dd')!;
    const copy = within(dd).getByRole('button', { name: 'ui.keyValue.copy' });
    await act(async () => { fireEvent.click(copy); });
    expect(writeText).toHaveBeenCalledWith('kronn/task/kt-1886');
    expect(within(dd).getByRole('status')).toHaveTextContent('ui.copy.copied');
    // Only the copyable item has one.
    expect(screen.getAllByRole('button')).toHaveLength(1);
  });

  it('copies copyText when the value is not plain text', async () => {
    render(<KeyValueList items={[{ label: 'Id', value: <code>abc</code>, copyable: true, copyText: 'abc-full' }]} />);
    await act(async () => { fireEvent.click(screen.getByRole('button')); });
    expect(writeText).toHaveBeenCalledWith('abc-full');
  });

  it('puts the label above its value in a narrow container, by a container query', () => {
    const { container } = render(<KeyValueList items={ITEMS} />);
    expect(container.firstElementChild).toHaveClass('ui-kv-frame');
    expect(CSS).toMatch(/\.ui-kv-frame\s*\{[^}]*container-type:\s*inline-size/);
    const query = CSS.match(/@container ui-kv \(width < 24rem\)\s*\{([\s\S]*)\}/)?.[1] ?? '';
    expect(query, 'a container query').not.toBe('');
    expect(query).toMatch(/\.ui-kv\[data-layout="grid"\]\s*\{[^}]*grid-template-columns:\s*minmax\(0,\s*1fr\)/);
    expect(query).toMatch(/\.ui-kv\[data-layout="inline"\] \.ui-kv-item\s*\{[^}]*flex-direction:\s*column/);
    expect(CSS).not.toMatch(/@media[^{]*width/);
  });

  it('passes axe in both layouts', async () => {
    render(<main><KeyValueList items={ITEMS} /><KeyValueList items={ITEMS} layout="inline" /></main>);
    const results = await act(() => runAxe(document.querySelector('main')!));
    expect(describeViolations(results)).toEqual([]);
  });
});
