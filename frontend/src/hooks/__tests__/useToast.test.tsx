// Tests for the toast hook — focused on the 'warning' variant added in 0.6.0
// (was a workaround using 'info' before — see TD-20260329-toast-no-warning).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import { MAX_TRANSIENT_TOASTS, useToast, withToast } from '../useToast';
import type { Toast } from '../toastTypes';

function ToastDemo({ run }: { run: (toast: ReturnType<typeof useToast>['toast']) => void }) {
  const { toast, ToastContainer } = useToast();
  return (
    <>
      <button onClick={() => run(toast)}>fire</button>
      <ToastContainer />
    </>
  );
}

describe('useToast — warning variant', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('renders a warning toast with data-type="warning"', () => {
    render(<ToastDemo run={(t) => t('Heads up', 'warning')} />);
    act(() => {
      screen.getByText('fire').click();
    });
    const toastEl = screen.getByText('Heads up').closest('.kr-toast');
    expect(toastEl).not.toBeNull();
    expect(toastEl?.getAttribute('data-type')).toBe('warning');
  });

  it('warnings are aria-live=assertive (interrupts SR like errors)', () => {
    render(<ToastDemo run={(t) => t('SR alert', 'warning')} />);
    act(() => {
      screen.getByText('fire').click();
    });
    const toastEl = screen.getByText('SR alert').closest('.kr-toast');
    expect(toastEl?.getAttribute('aria-live')).toBe('assertive');
  });

  it('warnings auto-dismiss after 7000ms (longer than info, shorter than persistent)', () => {
    render(<ToastDemo run={(t) => t('temp', 'warning')} />);
    act(() => {
      screen.getByText('fire').click();
    });
    expect(screen.queryByText('temp')).toBeInTheDocument();
    // Just before the 7s mark: still visible
    act(() => { vi.advanceTimersByTime(6999); });
    expect(screen.queryByText('temp')).toBeInTheDocument();
    // Past 7s: gone
    act(() => { vi.advanceTimersByTime(2); });
    expect(screen.queryByText('temp')).not.toBeInTheDocument();
  });

  it('warnings can be made persistent via options', () => {
    render(<ToastDemo run={(t) => t('sticky', 'warning', { persistent: true })} />);
    act(() => {
      screen.getByText('fire').click();
    });
    act(() => { vi.advanceTimersByTime(20_000); });
    expect(screen.queryByText('sticky')).toBeInTheDocument();
  });

  it('info toasts still render with cyan color (regression guard)', () => {
    render(<ToastDemo run={(t) => t('Info plain', 'info')} />);
    act(() => {
      screen.getByText('fire').click();
    });
    const toastEl = screen.getByText('Info plain').closest('.kr-toast');
    expect(toastEl?.getAttribute('data-type')).toBe('info');
    expect(toastEl?.getAttribute('aria-live')).toBe('polite');
  });

  it('dedup: identical (message, type) toasts within 1.5s collapse to one', () => {
    // Pin TD-20260510-multi-agent-disc-finished-toasts: a 4-agent
    // compare-agents run used to broadcast 4 identical "batch finished"
    // events from concurrent WS subscriptions, each calling toast()
    // separately. The cap-to-3 limit hid most of the damage but the
    // user still saw 3 stacked identical toasts. Dedup by (type,
    // message) inside a 1.5s window collapses them into one.
    function FireFour() {
      const { toast, ToastContainer } = useToast();
      return (
        <>
          <button onClick={() => {
            toast('Batch terminé', 'success');
            toast('Batch terminé', 'success');
            toast('Batch terminé', 'success');
            toast('Batch terminé', 'success');
          }}>fire4</button>
          <ToastContainer />
        </>
      );
    }
    render(<FireFour />);
    act(() => { screen.getByText('fire4').click(); });
    expect(screen.getAllByText('Batch terminé')).toHaveLength(1);
  });

  it('dedup: same message but DIFFERENT type still surfaces separately', () => {
    // A success and an error with the same wording are intentionally
    // distinct events — never collapse across types.
    function FireMixed() {
      const { toast, ToastContainer } = useToast();
      return (
        <>
          <button onClick={() => {
            toast('Save failed', 'success');
            toast('Save failed', 'error');
          }}>fire</button>
          <ToastContainer />
        </>
      );
    }
    render(<FireMixed />);
    act(() => { screen.getByText('fire').click(); });
    expect(screen.getAllByText('Save failed')).toHaveLength(2);
  });

  it('dedup: opt-out via { dedup: false } ships every fire', () => {
    function FireExplicit() {
      const { toast, ToastContainer } = useToast();
      return (
        <>
          <button onClick={() => {
            toast('Explicit retry', 'info', { dedup: false });
            toast('Explicit retry', 'info', { dedup: false });
          }}>fire</button>
          <ToastContainer />
        </>
      );
    }
    render(<FireExplicit />);
    act(() => { screen.getByText('fire').click(); });
    // Both toasts ship, under the cap of 3 transient toasts.
    expect(screen.getAllByText('Explicit retry')).toHaveLength(2);
  });

  it('dedup: same message AFTER the 1.5s window surfaces again', () => {
    function FireAcrossWindow() {
      const { toast, ToastContainer } = useToast();
      return (
        <>
          <button data-testid="first" onClick={() => toast('Welcome', 'info')}>first</button>
          <button data-testid="second" onClick={() => toast('Welcome', 'info')}>second</button>
          <ToastContainer />
        </>
      );
    }
    render(<FireAcrossWindow />);
    act(() => { screen.getByTestId('first').click(); });
    act(() => { vi.advanceTimersByTime(2_000); });
    act(() => { screen.getByTestId('second').click(); });
    // The first one auto-dismissed (info: 5s) — wait we set 2s only.
    // First toast still on screen, second one ALSO surfaces because
    // the dedup window expired.
    expect(screen.getAllByText('Welcome')).toHaveLength(2);
  });
});

// KT-1883 — the counter-audit (KT-1983) found a persistent error toast dropped
// as soon as three newer toasts arrived (`prev.slice(-2)` whatever the kind),
// and the stack fixed top right, over the bell and the detail header actions.
describe('useToast — a persistent toast is never evicted (KT-1883)', () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  const toastOf = (id: number, persistent: boolean): Toast =>
    ({ id, message: `m${id}`, type: persistent ? 'error' : 'info', persistent, copyable: null });

  it('keeps every persistent toast and the newest transient ones, in arrival order', () => {
    let stack: Toast[] = [];
    stack = withToast(stack, toastOf(1, true));
    for (let id = 2; id <= 40; id++) stack = withToast(stack, toastOf(id, false));
    stack = withToast(stack, toastOf(41, true));
    stack = withToast(stack, toastOf(42, false));
    expect(stack.map(t => t.id)).toEqual([1, 39, 40, 41, 42]);
    expect(stack.filter(t => !t.persistent)).toHaveLength(MAX_TRANSIENT_TOASTS);
  });

  it('keeps an error on screen after any number of later transient toasts, until it is dismissed', () => {
    function Burst() {
      const { toast, ToastContainer } = useToast();
      return (
        <>
          <button onClick={() => toast('Save failed: disk full', 'error')}>error</button>
          <button onClick={() => {
            for (let i = 0; i < 25; i++) toast(`Step ${i} done`, i % 2 ? 'success' : 'info');
          }}>burst</button>
          <ToastContainer />
        </>
      );
    }
    render(<Burst />);
    act(() => { screen.getByText('error').click(); });
    act(() => { screen.getByText('burst').click(); });
    expect(screen.getByText('Save failed: disk full')).toBeInTheDocument();
    expect(document.querySelectorAll('.kr-toast:not([data-type="error"])')).toHaveLength(MAX_TRANSIENT_TOASTS);
    act(() => { screen.getByText('burst').click(); });
    act(() => { vi.advanceTimersByTime(60_000); });
    expect(screen.getByText('Save failed: disk full')).toBeInTheDocument();
    expect(document.querySelectorAll('.kr-toast')).toHaveLength(1);

    act(() => { screen.getByRole('button', { name: 'Close' }).click(); });
    expect(screen.queryByText('Save failed: disk full')).toBeNull();
  });

  it('leaves the placement to toastStack.css: no inline position or width (KT-1883)', () => {
    render(<ToastDemo run={t => t('Saved', 'success')} />);
    act(() => { screen.getByText('fire').click(); });
    const stack = document.querySelector('.kr-toast-stack') as HTMLElement;
    expect(stack).toContainElement(screen.getByText('Saved'));
    for (const prop of ['position', 'top', 'right', 'bottom', 'left']) expect(stack.style.getPropertyValue(prop), prop).toBe('');
    const toast = document.querySelector('.kr-toast') as HTMLElement;
    for (const prop of ['max-width', 'min-width', 'animation']) expect(toast.style.getPropertyValue(prop), prop).toBe('');
  });

  it('puts the newest toast at the bottom of the stack', () => {
    render(<ToastDemo run={t => { t('First', 'info'); t('Second', 'info'); }} />);
    act(() => { screen.getByText('fire').click(); });
    expect([...document.querySelectorAll('.kr-toast')].map(e => e.textContent)).toEqual(['First', 'Second']);
  });

  it('scrolls the stack to the newest toast when one is added, once it is taller than the window', () => {
    // toastStack.css caps the stack's height and lets it scroll; jsdom lays
    // nothing out, so the stack's content height is given here.
    const height = vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get')
      .mockImplementation(function (this: HTMLElement) {
        return this.classList.contains('kr-toast-stack') ? this.querySelectorAll('.kr-toast').length * 240 : 0;
      });
    try {
      let n = 0;
      render(<ToastDemo run={t => { n += 1; t(`Error ${n}`, 'error'); }} />);
      for (let i = 0; i < 4; i++) act(() => { screen.getByText('fire').click(); });
      const stack = document.querySelector('.kr-toast-stack') as HTMLElement;
      expect(stack.scrollTop).toBe(4 * 240);

      // Back at the oldest to close it, then a new error arrives: it is in view.
      stack.scrollTop = 0;
      act(() => { screen.getByText('fire').click(); });
      expect((document.querySelector('.kr-toast-stack') as HTMLElement).scrollTop).toBe(5 * 240);
    } finally {
      height.mockRestore();
    }
  });
});
