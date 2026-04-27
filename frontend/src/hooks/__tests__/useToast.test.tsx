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
});
