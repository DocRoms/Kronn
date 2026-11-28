// KT-1886 — CopyButton: one copy action, named by its label, in an icon form
// and a text form. A copy is announced ("Copied", the icon turns into a check
// for about 1.5 s); a clipboard that refuses is announced too ("Copy failed",
// a danger state). Written before the component.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { CopyButton } from './CopyButton';
import { COPY_FEEDBACK_MS } from './dataKinds';
import { describeViolations, runAxe } from '../../test/axe';

const writeText = vi.fn<(text: string) => Promise<void>>();
const original = Object.getOwnPropertyDescriptor(navigator, 'clipboard');

beforeEach(() => {
  writeText.mockReset();
  writeText.mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
});

afterEach(() => {
  vi.useRealTimers();
  if (original) Object.defineProperty(navigator, 'clipboard', original);
  else delete (navigator as { clipboard?: unknown }).clipboard;
});

/** Click, then let the clipboard promise settle. */
async function press(button: HTMLElement) {
  await act(async () => { fireEvent.click(button); });
}

const live = () => document.querySelector('.ui-copy-button-live') as HTMLElement;

describe('CopyButton', () => {
  it('is an icon button named by its label, with type button and an empty polite live region', () => {
    render(<CopyButton text="kt-1886" label="Copy the task id" />);
    const button = screen.getByRole('button', { name: 'Copy the task id' });
    expect(button).toHaveAttribute('type', 'button');
    expect(button).toHaveClass('ui-icon-button');
    expect(button).toHaveAttribute('data-state', 'idle');
    expect(live()).toHaveAttribute('aria-live', 'polite');
    expect(live()).toHaveAttribute('role', 'status');
    expect(live()).toHaveTextContent('');
  });

  it('has a text form built on Button, its label written', () => {
    render(<CopyButton text="kt-1886" label="Copy the id" form="text" />);
    const button = screen.getByRole('button', { name: 'Copy the id' });
    expect(button).toHaveClass('ui-button');
    expect(button).toHaveTextContent('Copy the id');
  });

  it('copies its text, says "Copied" in the live region and shows a check, then goes back after ~1.5 s', async () => {
    vi.useFakeTimers();
    render(<CopyButton text="kt-1886" label="Copy the id" />);
    const button = screen.getByRole('button', { name: 'Copy the id' });
    await press(button);
    expect(writeText).toHaveBeenCalledWith('kt-1886');
    expect(button).toHaveAttribute('data-state', 'copied');
    expect(live()).toHaveTextContent('ui.copy.copied');
    expect(button.querySelector('[data-icon="check"]')).toBeTruthy();
    // The name does not move: the live region says what happened.
    expect(button).toHaveAccessibleName('Copy the id');

    act(() => { vi.advanceTimersByTime(COPY_FEEDBACK_MS - 100); });
    expect(button).toHaveAttribute('data-state', 'copied');
    act(() => { vi.advanceTimersByTime(200); });
    expect(button).toHaveAttribute('data-state', 'idle');
    expect(button.querySelector('[data-icon="copy"]')).toBeTruthy();
    expect(live()).toHaveTextContent('');
    expect(COPY_FEEDBACK_MS).toBe(1500);
  });

  it('reads getText at the moment of the click, sync or async', async () => {
    let n = 0;
    const { rerender } = render(<CopyButton getText={() => `value-${++n}`} label="Copy" />);
    await press(screen.getByRole('button', { name: 'Copy' }));
    expect(writeText).toHaveBeenLastCalledWith('value-1');
    rerender(<CopyButton getText={async () => 'later'} label="Copy" />);
    await press(screen.getByRole('button', { name: 'Copy' }));
    expect(writeText).toHaveBeenLastCalledWith('later');
  });

  it('says "Copy failed" and turns danger when the clipboard refuses', async () => {
    writeText.mockRejectedValue(new DOMException('denied', 'NotAllowedError'));
    render(<CopyButton text="secret-less" label="Copy the path" />);
    const button = screen.getByRole('button', { name: 'Copy the path' });
    await press(button);
    expect(button).toHaveAttribute('data-state', 'failed');
    expect(button).toHaveAttribute('data-variant', 'danger');
    expect(live()).toHaveTextContent('ui.copy.failed');
    expect(button.querySelector('[data-icon="failed"]')).toBeTruthy();
  });

  it('fails the same way when there is no clipboard at all, or getText throws', async () => {
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
    const { rerender } = render(<CopyButton text="x" label="Copy" form="text" />);
    await press(screen.getByRole('button', { name: 'Copy' }));
    expect(live()).toHaveTextContent('ui.copy.failed');
    expect(screen.getByRole('button')).toHaveAttribute('data-variant', 'danger');

    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    rerender(<CopyButton key="b" getText={() => { throw new Error('gone'); }} label="Copy" form="text" />);
    await press(screen.getByRole('button', { name: 'Copy' }));
    expect(writeText).not.toHaveBeenCalled();
    expect(live()).toHaveTextContent('ui.copy.failed');
  });

  it('clears its timer when it unmounts', async () => {
    vi.useFakeTimers();
    const { unmount } = render(<CopyButton text="x" label="Copy" />);
    await press(screen.getByRole('button', { name: 'Copy' }));
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('passes axe in every state', async () => {
    writeText.mockRejectedValueOnce(new Error('no'));
    render(<main><CopyButton text="a" label="Copy a" /><CopyButton text="b" label="Copy b" form="text" /></main>);
    await press(screen.getByRole('button', { name: 'Copy a' }));
    await press(screen.getByRole('button', { name: 'Copy b' }));
    const results = await act(() => runAxe(document.querySelector('main')!));
    expect(describeViolations(results)).toEqual([]);
  });
});
