// KT-2004 — LogViewer: one log of lines. The scroll area is a focusable
// region named by its required label with role="log"; stderr and error
// lines carry a text marker besides their tint; a time is a <time> of its own;
// follow keeps the end in view until the reader scrolls up, then a "jump to the
// end" control resumes it; wrap or sideways scroll with a toggle; a toolbar
// that shows only what is wired (copy, refresh, clear); a note slot; an empty
// text; ANSI escape sequences never reach the screen. Written before the
// component.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { act } from 'react';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LogViewer } from './LogViewer';
import { stripAnsi, type LogLine } from './logKinds';
import { cssText } from '../../test/cssBlocks';
import { describeViolations, runAxe } from '../../test/axe';

const CSS = cssText(join(dirname(fileURLToPath(import.meta.url)), 'LogViewer.css'));

const LINES: LogLine[] = [
  'Cloning repository…',
  { text: 'npm WARN deprecated glob@7', stream: 'stderr' },
  { text: 'Build failed', level: 'error', time: '2026-11-22T10:15:30Z' },
  { text: 'Retrying', level: 'info', time: new Date('2026-11-22T10:15:31Z') },
];

const log = () => screen.getByRole('log', { name: 'Run output' });

/** jsdom lays nothing out: give the scroll area a height and a scroll top. */
function layout(el: HTMLElement, { scrollHeight, clientHeight }: { scrollHeight: number; clientHeight: number }) {
  let top = 0;
  Object.defineProperty(el, 'scrollHeight', { configurable: true, get: () => scrollHeight });
  Object.defineProperty(el, 'clientHeight', { configurable: true, get: () => clientHeight });
  Object.defineProperty(el, 'scrollTop', { configurable: true, get: () => top, set: (v: number) => { top = v; } });
  return {
    setScrollHeight: (h: number) => { scrollHeight = h; },
    scrollTo: (v: number) => { top = v; fireEvent.scroll(el); },
  };
}

describe('LogViewer', () => {
  it('is a focusable region named by its label, with role log', () => {
    render(<LogViewer label="Run output" lines={LINES} />);
    expect(log()).toHaveAttribute('tabindex', '0');
    expect(log()).toHaveAttribute('aria-label', 'Run output');
    expect(log().querySelectorAll('.ui-log-line')).toHaveLength(4);
  });

  it('tints a stderr line and an error line and gives each a written marker', () => {
    render(<LogViewer label="Run output" lines={LINES} />);
    const [plain, stderr, error, info] = [...log().querySelectorAll('.ui-log-line')] as HTMLElement[];
    expect(plain).not.toHaveAttribute('data-tone');
    expect(plain.querySelector('.ui-log-marker')).toBeNull();
    expect(stderr).toHaveAttribute('data-stream', 'stderr');
    expect(stderr).toHaveAttribute('data-tone', 'danger');
    expect(within(stderr).getByText('ui.log.stderr')).toHaveClass('ui-log-marker');
    expect(error).toHaveAttribute('data-level', 'error');
    expect(error).toHaveAttribute('data-tone', 'danger');
    expect(within(error).getByText('ui.log.level.error')).toHaveClass('ui-log-marker');
    expect(info).not.toHaveAttribute('data-tone');
    // The tint comes from the status tokens.
    expect(CSS).toMatch(/\.ui-log-line\[data-tone="danger"\][^{]*\{[^}]*var\(--kr-status-danger\)/);
  });

  it('writes a time in a <time> of its own column', () => {
    render(<LogViewer label="Run output" lines={LINES} />);
    const times = log().querySelectorAll('time.ui-log-time');
    expect(times).toHaveLength(2);
    expect(times[0]).toHaveAttribute('datetime', '2026-11-22T10:15:30Z');
    expect(times[1]).toHaveAttribute('datetime', '2026-11-22T10:15:31.000Z');
    expect(times[1].textContent).not.toBe('');
  });

  it('removes ANSI escape sequences from what it shows and copies', () => {
    const raw = '\u001b[31mred\u001b[0m and \u001b[1;32mbold green\u001b[39;22m \u001b]8;;https://x\u0007link\u001b]8;;\u0007';
    expect(stripAnsi(raw)).toBe('red and bold green link');
    render(<LogViewer label="Run output" lines={[raw]} />);
    expect(log()).toHaveTextContent('red and bold green link');
    expect(log().textContent).not.toContain('\u001b');
  });

  it('shows the empty text when there is no line, a default one otherwise', () => {
    const { rerender } = render(<LogViewer label="Run output" lines={[]} />);
    expect(within(log()).getByText('ui.log.empty')).toBeInTheDocument();
    rerender(<LogViewer label="Run output" lines={[]} empty="No output yet" />);
    expect(within(log()).getByText('No output yet')).toBeInTheDocument();
  });

  it('shows a note ("last N lines", "truncated") outside the log', () => {
    render(<LogViewer label="Run output" lines={LINES} note="Last 4 lines" />);
    const note = screen.getByText('Last 4 lines');
    expect(note.closest('.ui-log-note')).toBeTruthy();
    expect(log()).not.toContainElement(note);
  });

  it('numbers the lines when asked, without reading the numbers into the text', () => {
    const { rerender } = render(<LogViewer label="Run output" lines={LINES} />);
    expect(log().querySelector('.ui-log-number')).toBeNull();
    rerender(<LogViewer label="Run output" lines={LINES} lineNumbers />);
    const numbers = [...log().querySelectorAll('.ui-log-number')];
    expect(numbers.map(n => n.textContent)).toEqual(['1', '2', '3', '4']);
    expect(numbers[0]).toHaveAttribute('aria-hidden', 'true');
  });

  it('numbers a tail from its startNumber, the column as wide as the last number', () => {
    const { container, rerender } = render(<LogViewer label="Run output" lines={LINES} lineNumbers />);
    const root = () => container.querySelector('.ui-log') as HTMLElement;
    expect(root().style.getPropertyValue('--ui-log-digits')).toBe('1');
    rerender(<LogViewer label="Run output" lines={LINES} lineNumbers startNumber={97} />);
    const numbers = [...log().querySelectorAll('.ui-log-number')];
    expect(numbers.map(n => n.textContent)).toEqual(['97', '98', '99', '100']);
    expect(root().style.getPropertyValue('--ui-log-digits')).toBe('3');
  });

  it('draws a cursor on the last line while streaming', () => {
    const { rerender } = render(<LogViewer label="Run output" lines={LINES} streaming />);
    const cursor = log().querySelector('.ui-log-cursor')!;
    expect(cursor).toHaveAttribute('aria-hidden', 'true');
    expect(cursor.closest('.ui-log-line')).toBe(log().querySelectorAll('.ui-log-line')[3]);
    rerender(<LogViewer label="Run output" lines={LINES} />);
    expect(log().querySelector('.ui-log-cursor')).toBeNull();
  });

  it('takes a height preset or fills its parent', () => {
    const { container, rerender } = render(<LogViewer label="Run output" lines={LINES} />);
    expect(container.querySelector('.ui-log')).toHaveAttribute('data-max-height', 'md');
    for (const preset of ['sm', 'lg', 'fill'] as const) {
      rerender(<LogViewer label="Run output" lines={LINES} maxHeight={preset} />);
      expect(container.querySelector('.ui-log')).toHaveAttribute('data-max-height', preset);
    }
    expect(CSS).toMatch(/\.ui-log\[data-max-height="fill"\][^{]*\{[^}]*height:\s*100%/);
  });

  describe('wrap', () => {
    it('wraps by default; the toggle switches to sideways scroll and back', async () => {
      const user = userEvent.setup();
      const { container } = render(<LogViewer label="Run output" lines={LINES} />);
      const root = container.querySelector('.ui-log')!;
      const toggle = screen.getByRole('button', { name: 'ui.log.wrap' });
      expect(root).toHaveAttribute('data-wrap', 'true');
      expect(toggle).toHaveAttribute('aria-pressed', 'true');
      await user.click(toggle);
      expect(root).toHaveAttribute('data-wrap', 'false');
      expect(toggle).toHaveAttribute('aria-pressed', 'false');
      await user.click(toggle);
      expect(root).toHaveAttribute('data-wrap', 'true');
    });

    it('starts on sideways scroll with wrap={false}', () => {
      const { container } = render(<LogViewer label="Run output" lines={LINES} wrap={false} />);
      expect(container.querySelector('.ui-log')).toHaveAttribute('data-wrap', 'false');
      expect(CSS).toMatch(/\.ui-log\[data-wrap="false"\] \.ui-log-text\s*\{[^}]*white-space:\s*pre;/);
    });
  });

  describe('follow', () => {
    it('keeps the end in view as lines arrive, stops on a manual scroll up, and resumes from the jump control', async () => {
      const user = userEvent.setup();
      const { rerender } = render(<LogViewer label="Run output" lines={LINES.slice(0, 2)} />);
      const box = layout(log(), { scrollHeight: 500, clientHeight: 100 });

      rerender(<LogViewer label="Run output" lines={LINES.slice(0, 3)} />);
      expect(log().scrollTop).toBe(500);
      expect(screen.queryByRole('button', { name: 'ui.log.jumpToEnd' })).toBeNull();

      // The reader scrolls up: follow turns off, the jump control shows.
      box.scrollTo(120);
      const jump = screen.getByRole('button', { name: 'ui.log.jumpToEnd' });
      box.setScrollHeight(800);
      rerender(<LogViewer label="Run output" lines={LINES} />);
      expect(log().scrollTop).toBe(120);

      await user.click(jump);
      expect(log().scrollTop).toBe(800);
      expect(screen.queryByRole('button', { name: 'ui.log.jumpToEnd' })).toBeNull();
      expect(log()).toHaveFocus();

      box.setScrollHeight(900);
      rerender(<LogViewer label="Run output" lines={[...LINES, 'more']} />);
      expect(log().scrollTop).toBe(900);
    });

    it('resumes when the reader scrolls back to the end by hand', () => {
      render(<LogViewer label="Run output" lines={LINES} />);
      const box = layout(log(), { scrollHeight: 500, clientHeight: 100 });
      box.scrollTo(100);
      expect(screen.getByRole('button', { name: 'ui.log.jumpToEnd' })).toBeInTheDocument();
      box.scrollTo(400);
      expect(screen.queryByRole('button', { name: 'ui.log.jumpToEnd' })).toBeNull();
    });

    it('never scrolls with follow={false}', () => {
      const { rerender } = render(<LogViewer label="Run output" lines={LINES.slice(0, 2)} follow={false} />);
      layout(log(), { scrollHeight: 500, clientHeight: 100 });
      rerender(<LogViewer label="Run output" lines={LINES} follow={false} />);
      expect(log().scrollTop).toBe(0);
    });
  });

  describe('toolbar', () => {
    const original = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    let writeText: ReturnType<typeof vi.fn>;
    beforeEach(() => {
      writeText = vi.fn().mockResolvedValue(undefined);
      Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    });
    afterEach(() => {
      if (original) Object.defineProperty(navigator, 'clipboard', original);
      else delete (navigator as { clipboard?: unknown }).clipboard;
    });

    it('shows only the wrap toggle when nothing else is wired', () => {
      const { container } = render(<LogViewer label="Run output" lines={LINES} />);
      const toolbar = container.querySelector('.ui-log-toolbar') as HTMLElement;
      expect(within(toolbar).getAllByRole('button').map(b => b.getAttribute('data-action'))).toEqual(['wrap']);
    });

    it('copies the whole text, ANSI removed, one line per line', async () => {
      render(<LogViewer label="Run output" lines={['\u001b[32mok\u001b[0m', { text: 'boom', stream: 'stderr' }]} copyable />);
      await act(async () => { screen.getByRole('button', { name: 'ui.log.copy' }).click(); });
      expect(writeText).toHaveBeenCalledWith('ok\nboom');
    });

    it('wires refresh and clear to their callbacks', async () => {
      const user = userEvent.setup();
      const onRefresh = vi.fn();
      const onClear = vi.fn();
      render(<LogViewer label="Run output" lines={LINES} onRefresh={onRefresh} onClear={onClear} copyable />);
      await user.click(screen.getByRole('button', { name: 'ui.log.refresh' }));
      await user.click(screen.getByRole('button', { name: 'ui.log.clear' }));
      expect(onRefresh).toHaveBeenCalledTimes(1);
      expect(onClear).toHaveBeenCalledTimes(1);
    });
  });

  it('writes no text under --kr-type-sm and keeps a mono font from the tokens', () => {
    for (const [, value] of CSS.matchAll(/font-size:\s*([^;]+);/g)) {
      expect(value.trim()).toMatch(/^var\(--kr-type-(sm|md|lg|xl|2xl)\)$/);
    }
    expect(CSS).toMatch(/font-family:\s*var\(--kr-font-mono\)/);
  });

  it('passes axe with every option on', async () => {
    const { container } = render(
      <main>
        <LogViewer label="Run output" lines={LINES} note="Last 4 lines" lineNumbers streaming copyable onRefresh={() => {}} onClear={() => {}} />
      </main>,
    );
    const results = await act(() => runAxe(container));
    expect(describeViolations(results)).toEqual([]);
  });
});
