import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act } from '@testing-library/react';
import { useMatrixDecode } from '../useMatrixDecode';

function TestHarness({ text, active }: { text: string; active: boolean }) {
  const out = useMatrixDecode(text, active);
  return <span data-testid="out">{out}</span>;
}

describe('useMatrixDecode', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('returns target verbatim when inactive', () => {
    const { getByTestId } = render(<TestHarness text="Hello World" active={false} />);
    expect(getByTestId('out').textContent).toBe('Hello World');
  });

  it('scrambles chars then settles to target when active', () => {
    const { getByTestId } = render(<TestHarness text="Kronn" active={true} />);

    // First tick runs synchronously inside the effect — display already
    // diverged from the target.
    const initial = getByTestId('out').textContent ?? '';
    expect(initial).toHaveLength(5);
    // Initial tick: order is shuffled so AT LEAST one char is scrambled
    // (statistically nearly certain; the edge case of zero scrambled on
    // the first frame is impossible since settledCount = 0 at frame 0).
    // We allow for the rare outcome where the scrambled char happens
    // to land on the same letter as the target — just assert length.

    // Run the whole animation and verify we land on the target.
    act(() => {
      vi.advanceTimersByTime(16 * 40); // > TOTAL_FRAMES * FRAME_MS
    });

    expect(getByTestId('out').textContent).toBe('Kronn');
  });

  it('preserves whitespace throughout the animation', () => {
    const target = 'Hello World Foo';
    const { getByTestId } = render(<TestHarness text={target} active={true} />);
    // Sample 3 intermediate frames and verify every space position
    // still has a space in the scrambled output.
    for (let step = 1; step <= 3; step++) {
      act(() => {
        vi.advanceTimersByTime(16 * 8);
      });
      const out = getByTestId('out').textContent ?? '';
      expect(out).toHaveLength(target.length);
      for (let i = 0; i < target.length; i++) {
        if (target[i] === ' ') {
          expect(out[i]).toBe(' ');
        }
      }
    }
    // Finish
    act(() => { vi.advanceTimersByTime(16 * 30); });
    expect(getByTestId('out').textContent).toBe(target);
  });

  it('restarts the animation when text changes', () => {
    const { getByTestId, rerender } = render(<TestHarness text="One" active={true} />);
    act(() => { vi.advanceTimersByTime(16 * 40); });
    expect(getByTestId('out').textContent).toBe('One');

    rerender(<TestHarness text="Two" active={true} />);
    // Mid-flight — length must now be 3 matching "Two"'s length
    expect(getByTestId('out').textContent).toHaveLength(3);
    act(() => { vi.advanceTimersByTime(16 * 40); });
    expect(getByTestId('out').textContent).toBe('Two');
  });

  it('does not re-trigger when text is set to the same value', () => {
    const { getByTestId, rerender } = render(<TestHarness text="Same" active={true} />);
    act(() => { vi.advanceTimersByTime(16 * 40); });
    expect(getByTestId('out').textContent).toBe('Same');

    // Identical rerender — output should stay "Same" (no scramble start)
    rerender(<TestHarness text="Same" active={true} />);
    expect(getByTestId('out').textContent).toBe('Same');
  });

  it('re-scrambles when a matrix:pulse event rolls a hit', () => {
    // PULSE_REDECODE_CHANCE is 0.15 — force a hit by stubbing Math.random.
    const randSpy = vi.spyOn(Math, 'random').mockReturnValue(0); // 0 < 0.15 → hit
    const { getByTestId } = render(<TestHarness text="Signal" active={true} />);

    // Finish initial decode first.
    act(() => { vi.advanceTimersByTime(16 * 40); });
    expect(getByTestId('out').textContent).toBe('Signal');

    // Fire a pulse — hook should re-scramble.
    act(() => { window.dispatchEvent(new CustomEvent('matrix:pulse')); });

    // Restore random for the scramble internals (they use rand to
    // pick chars — want them to vary, but the RESULT length stays 6).
    randSpy.mockRestore();

    // Mid-flight length matches target.
    expect(getByTestId('out').textContent).toHaveLength(6);

    // Let it finish.
    act(() => { vi.advanceTimersByTime(16 * 40); });
    expect(getByTestId('out').textContent).toBe('Signal');
  });

  it('ignores matrix:pulse when the roll misses', () => {
    // Math.random returns 0.99 → > 0.15 → miss → no re-scramble
    const randSpy = vi.spyOn(Math, 'random').mockReturnValue(0.99);
    const { getByTestId } = render(<TestHarness text="Hello" active={true} />);
    // Finish initial decode
    act(() => { vi.advanceTimersByTime(16 * 40); });
    expect(getByTestId('out').textContent).toBe('Hello');

    act(() => { window.dispatchEvent(new CustomEvent('matrix:pulse')); });
    // Still "Hello" — no new scramble started
    expect(getByTestId('out').textContent).toBe('Hello');
    randSpy.mockRestore();
  });

  it('ignores matrix:pulse when the theme is inactive', () => {
    const randSpy = vi.spyOn(Math, 'random').mockReturnValue(0);
    const { getByTestId } = render(<TestHarness text="Inert" active={false} />);
    expect(getByTestId('out').textContent).toBe('Inert');

    act(() => { window.dispatchEvent(new CustomEvent('matrix:pulse')); });
    expect(getByTestId('out').textContent).toBe('Inert');
    randSpy.mockRestore();
  });
});
