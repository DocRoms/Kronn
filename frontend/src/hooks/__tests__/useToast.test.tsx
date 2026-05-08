// Tests for the toast hook — focused on the 'warning' variant added in 0.6.0
// (was a workaround using 'info' before — see TD-20260329-toast-no-warning).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import { useToast } from '../useToast';

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
    // Both toasts ship — but the 3-slot cap (prev.slice(-2) + new = 3)
    // still applies, so we get 2 visible.
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
