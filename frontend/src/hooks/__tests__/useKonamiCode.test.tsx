import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { act } from 'react';
import { useKonamiCode } from '../useKonamiCode';

function TestHarness({ onUnlock }: { onUnlock: () => void }) {
  useKonamiCode(onUnlock);
  return null;
}

function fireKey(key: string, target: EventTarget = window) {
  const ev = new KeyboardEvent('keydown', { key, bubbles: true });
  // For target-specific dispatch we need the element itself.
  if (target !== window) {
    target.dispatchEvent(ev);
  } else {
    window.dispatchEvent(ev);
  }
}

const KONAMI = [
  'ArrowUp', 'ArrowUp',
  'ArrowDown', 'ArrowDown',
  'ArrowLeft', 'ArrowRight',
  'ArrowLeft', 'ArrowRight',
  'b', 'a',
];

describe('useKonamiCode', () => {
  it('fires onUnlock when the full Konami sequence is typed', () => {
    const onUnlock = vi.fn();
    render(<TestHarness onUnlock={onUnlock} />);

    act(() => {
      for (const k of KONAMI) fireKey(k);
    });

    expect(onUnlock).toHaveBeenCalledTimes(1);
  });

  it('does NOT fire on a partial sequence', () => {
    const onUnlock = vi.fn();
    render(<TestHarness onUnlock={onUnlock} />);

    act(() => {
      // 9 of 10 keys
      for (const k of KONAMI.slice(0, -1)) fireKey(k);
    });

    expect(onUnlock).not.toHaveBeenCalled();
  });

  it('resets on wrong key but a fresh sequence still works after', () => {
    const onUnlock = vi.fn();
    render(<TestHarness onUnlock={onUnlock} />);

    act(() => {
      fireKey('ArrowUp');
      fireKey('ArrowDown'); // wrong — resets
      // now do the full sequence cleanly
      for (const k of KONAMI) fireKey(k);
    });

    expect(onUnlock).toHaveBeenCalledTimes(1);
  });

  it('accepts uppercase B/A (shift held)', () => {
    const onUnlock = vi.fn();
    render(<TestHarness onUnlock={onUnlock} />);

    act(() => {
      for (const k of KONAMI.slice(0, -2)) fireKey(k);
      fireKey('B');
      fireKey('A');
    });

    expect(onUnlock).toHaveBeenCalledTimes(1);
  });

  it('ignores keys while the user is typing in an INPUT', () => {
    const onUnlock = vi.fn();
    render(
      <>
        <TestHarness onUnlock={onUnlock} />
        <input data-testid="typed-input" />
      </>
    );
    const input = document.querySelector('input') as HTMLInputElement;
    input.focus();

    act(() => {
      // Dispatch the sequence with input as target — must be ignored
      for (const k of KONAMI) fireKey(k, input);
    });

    expect(onUnlock).not.toHaveBeenCalled();
  });

  it('fires repeatedly — second sequence works after the first', () => {
    const onUnlock = vi.fn();
    render(<TestHarness onUnlock={onUnlock} />);

    act(() => {
      for (const k of KONAMI) fireKey(k);
      for (const k of KONAMI) fireKey(k);
    });

    expect(onUnlock).toHaveBeenCalledTimes(2);
  });
});
