// A task description is read far more often than it is edited, so it renders
// as Markdown by default and a switch exposes the raw, editable source.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { TaskDescription } from '../TaskDescription';
import { TASK_DESCRIPTION_MODE_KEY } from '../../lib/taskDescriptionMode';

const MARKDOWN = [
  '# Goal',
  '',
  '- first',
  '- second',
  '',
  '| Col | Value |',
  '| --- | ----- |',
  '| a   | 1     |',
  '',
  'Run `make test` or read [the docs](https://example.com).',
].join('\n');

function Editable({ initial, onValue }: { initial: string; onValue?: (value: string) => void }) {
  const [value, setValue] = useState(initial);
  return (
    <TaskDescription
      value={value}
      onChange={next => {
        setValue(next);
        onValue?.(next);
      }}
    />
  );
}

describe('TaskDescription', () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => cleanup());

  it('renders the description as Markdown by default', () => {
    const { container } = render(<Editable initial={MARKDOWN} />);
    expect(screen.getByRole('heading', { level: 1, name: 'Goal' })).toBeInTheDocument();
    expect(container.querySelector('.task-description-rendered table')).not.toBeNull();
    expect(screen.getAllByRole('listitem')).toHaveLength(2);
    expect(screen.getByText('make test').tagName).toBe('CODE');
    expect(screen.getByRole('link', { name: 'the docs' })).toHaveAttribute('href', 'https://example.com');
    expect(container.textContent).not.toContain('# Goal');
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.getByRole('switch', { name: 'planning.descriptionRawHint' }))
      .toHaveAttribute('aria-checked', 'false');
  });

  it('switches to the raw source, which is editable, and keeps the edit when switching back', () => {
    const onValue = vi.fn();
    render(<Editable initial={MARKDOWN} onValue={onValue} />);
    fireEvent.click(screen.getByRole('switch'));
    const textarea = screen.getByRole('textbox', { name: 'planning.description' });
    expect(textarea).toHaveValue(MARKDOWN);
    fireEvent.change(textarea, { target: { value: '## Edited' } });
    expect(onValue).toHaveBeenLastCalledWith('## Edited');

    fireEvent.click(screen.getByRole('switch'));
    expect(screen.getByRole('heading', { level: 2, name: 'Edited' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('switch'));
    expect(screen.getByRole('textbox')).toHaveValue('## Edited');
  });

  it('remembers the chosen mode across mounts', () => {
    const first = render(<Editable initial={MARKDOWN} />);
    fireEvent.click(screen.getByRole('switch'));
    expect(localStorage.getItem(TASK_DESCRIPTION_MODE_KEY)).toBe('raw');
    first.unmount();

    render(<Editable initial={MARKDOWN} />);
    expect(screen.getByRole('switch')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('textbox')).toHaveValue(MARKDOWN);
  });

  it('falls back to the rendered view when storage throws', () => {
    const getItem = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    const setItem = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    try {
      render(<Editable initial={MARKDOWN} />);
      expect(screen.getByRole('heading', { level: 1 })).toBeInTheDocument();
      fireEvent.click(screen.getByRole('switch'));
      expect(screen.getByRole('textbox')).toHaveValue(MARKDOWN);
    } finally {
      getItem.mockRestore();
      setItem.mockRestore();
    }
  });

  it('never executes raw HTML from a description', () => {
    const payload = '<img src="x" onerror="window.__taskXss = 1"><script>window.__taskXss = 2</script>\n\n[bad](javascript:alert(1))';
    const { container } = render(<Editable initial={payload} />);
    expect(container.querySelector('.task-description-rendered img')).toBeNull();
    expect(container.querySelector('.task-description-rendered script')).toBeNull();
    const link = screen.queryByRole('link', { name: 'bad' });
    expect(link?.getAttribute('href') ?? '').not.toMatch(/^javascript:/i);
    expect((window as unknown as { __taskXss?: number }).__taskXss).toBeUndefined();
  });

  it('offers to write an empty description from the rendered view', () => {
    render(<Editable initial="" />);
    expect(screen.getByText('planning.descriptionEmpty')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'planning.descriptionWrite' }));
    expect(screen.getByRole('textbox')).toHaveValue('');
  });

  it('shows the raw source read-only when no editor is wired', () => {
    const { container } = render(<TaskDescription value={MARKDOWN} />);
    expect(screen.getByRole('switch', { name: 'planning.descriptionRawHintReadOnly' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('switch'));
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(container.querySelector('pre.task-description-source')?.textContent).toBe(MARKDOWN);
  });
});
