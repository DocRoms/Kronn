// KT-1684 — server errors and an unreachable backend live in the bell: no stack
// of toasts, at most one short "see in the bell". 4xx stay next to the action.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { resetServerFailureToast, useToast } from '../useToast';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

function Demo({ messages }: { messages: string[] }) {
  const { toast, ToastContainer } = useToast();
  return (
    <>
      <button onClick={() => messages.forEach(message => toast(message, 'error'))}>fire</button>
      <ToastContainer />
    </>
  );
}

function fire(messages: string[]) {
  render(<I18nProvider><Demo messages={messages} /></I18nProvider>);
  act(() => { screen.getByText('fire').click(); });
}

beforeEach(() => { vi.useFakeTimers(); resetServerFailureToast(); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe('useToast — server failures', () => {
  it('collapses a burst of 5xx into one short toast that points at the bell', () => {
    fire([
      'Server error (HTTP 502)',
      'Server error (HTTP 500) — boom',
      'Server error (HTTP 503) — <html>Bad Gateway</html>',
      'Impossible de contacter le serveur. Vérifiez votre connexion.',
    ]);
    const toasts = screen.getAllByRole('alert');
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toHaveTextContent('Problème serveur — voir la cloche.');
    expect(toasts[0]).not.toHaveTextContent('502');
    // Short: it goes away by itself, unlike a persistent error toast.
    act(() => { vi.advanceTimersByTime(8_000); });
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('does not repeat it for the next failures within the window, wherever they are fired from', () => {
    fire(['Server error (HTTP 500) — one']);
    act(() => { vi.advanceTimersByTime(8_000); });
    expect(screen.queryByRole('alert')).toBeNull();
    act(() => { screen.getByText('fire').click(); });
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('leaves a 4xx and any other error as it was: persistent, next to the action', () => {
    fire(['Server error (HTTP 422) — missing field "name"', 'Project path does not exist']);
    const toasts = screen.getAllByRole('alert');
    expect(toasts).toHaveLength(2);
    expect(toasts[0]).toHaveTextContent('HTTP 422');
    act(() => { vi.advanceTimersByTime(60_000); });
    expect(screen.getAllByRole('alert')).toHaveLength(2);
  });
});
