/**
 * KT-561 — deleting in two steps, where the control lives.
 *
 * What this file guards is not the happy path: it is the two ways the previous
 * code went silent. A native `confirm()` that never shows answers `false`, so
 * the click did nothing at all; and the request was awaited with no error
 * handling, so a server refusal left no trace on screen either. Both looked
 * exactly like "deletion does not work".
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { act, render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { ConfirmDeleteButton } from '../ConfirmDeleteButton';

afterEach(cleanup);

function renderButton(onConfirm: () => Promise<void>, onError?: (m: string) => void) {
  render(
    <ConfirmDeleteButton
      onConfirm={onConfirm}
      label="Supprimer"
      confirmLabel="Confirmer"
      itemName="Mon workflow"
      testId="delete-it"
      onError={onError}
    />,
  );
  return screen.getByTestId('delete-it');
}

describe('ConfirmDeleteButton', () => {
  it('arms before it acts, and never on the first click', () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    const button = renderButton(onConfirm);

    fireEvent.click(button);
    expect(onConfirm).not.toHaveBeenCalled();
    // The armed state is visible AND announced: the next click destroys
    // something, and a screen reader must say which one.
    expect(button).toHaveAttribute('data-armed', 'true');
    expect(button).toHaveAccessibleName('Confirmer · Mon workflow');

    fireEvent.click(button);
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it('keeps the object on screen when the server refuses, and says why', async () => {
    const onError = vi.fn();
    const button = renderButton(
      () => Promise.reject(new Error('workflow is still running')),
      onError,
    );

    fireEvent.click(button);
    fireEvent.click(button);

    expect(await screen.findByTestId('delete-it-error'))
      .toHaveTextContent('workflow is still running');
    expect(onError).toHaveBeenCalledWith('workflow is still running');
    // Disarmed, so a stray click cannot retry destruction by accident.
    await waitFor(() => expect(button).toHaveAttribute('data-armed', 'false'));
  });

  it('disarms itself rather than waiting for a click aimed elsewhere', () => {
    vi.useFakeTimers();
    try {
      const onConfirm = vi.fn().mockResolvedValue(undefined);
      const button = renderButton(onConfirm);
      fireEvent.click(button);
      expect(button).toHaveAttribute('data-armed', 'true');

      act(() => { vi.advanceTimersByTime(5001); });
      expect(button).toHaveAttribute('data-armed', 'false');

      // The next click arms again instead of deleting.
      fireEvent.click(button);
      expect(onConfirm).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('cannot be fired twice while the request is in flight', async () => {
    const deferred: { resolve?: () => void } = {};
    const onConfirm = vi.fn(() => new Promise<void>(resolve => { deferred.resolve = resolve; }));
    const button = renderButton(onConfirm);

    fireEvent.click(button);
    fireEvent.click(button);
    expect(button).toBeDisabled();
    // A third click while the request is still out must not schedule a second
    // deletion of the same thing.
    fireEvent.click(button);
    expect(onConfirm).toHaveBeenCalledTimes(1);

    await act(async () => { deferred.resolve?.(); });
    expect(button).not.toBeDisabled();
  });
  // KT-561 DoD 3 — what the deletion takes with it is said while armed, not
  // discovered after. The four things that matter: it is absent before the
  // first click, present and announced once armed, still readable when the
  // count arrives late, and never silently missing when the count fails.
  describe('impact', () => {
    function renderWithImpact(
      impact: () => string | null | Promise<string | null>,
      impactUnknown?: string,
    ) {
      render(
        <ConfirmDeleteButton
          onConfirm={vi.fn().mockResolvedValue(undefined)}
          label="Supprimer"
          confirmLabel="Confirmer"
          itemName="Mon workflow"
          testId="delete-it"
          impact={impact}
          impactUnknown={impactUnknown}
        />,
      );
      return screen.getByTestId('delete-it');
    }

    it('says what goes with the object once armed, and describes the button with it', () => {
      const button = renderWithImpact(() => '3 exécutions supprimées avec lui');
      expect(screen.queryByTestId('delete-it-impact')).toBeNull();

      fireEvent.click(button);
      expect(screen.getByTestId('delete-it-impact'))
        .toHaveTextContent('3 exécutions supprimées avec lui');
      expect(button).toHaveAccessibleDescription('3 exécutions supprimées avec lui');
    });

    it('shows a count that arrives late, and restarts the disarm window for it', async () => {
      vi.useFakeTimers();
      try {
        const button = renderWithImpact(
          () => new Promise(resolve => { window.setTimeout(() => resolve('12 exécutions'), 4000); }),
        );
        fireEvent.click(button);
        expect(screen.queryByTestId('delete-it-impact')).toBeNull();

        await act(async () => { await vi.advanceTimersByTimeAsync(4000); });
        expect(screen.getByTestId('delete-it-impact')).toHaveTextContent('12 exécutions');

        // 8 s after arming: the original 5 s window is over, the restarted one is not.
        await act(async () => { await vi.advanceTimersByTimeAsync(4000); });
        expect(button).toHaveAttribute('data-armed', 'true');
        await act(async () => { await vi.advanceTimersByTimeAsync(1500); });
        expect(button).toHaveAttribute('data-armed', 'false');
      } finally {
        vi.useRealTimers();
      }
    });

    it('reads as unknown when the count cannot be fetched, never as nothing', async () => {
      const button = renderWithImpact(
        () => Promise.reject(new Error('502')),
        'Impact inconnu',
      );
      // The rejection settles in a microtask after the click: same act.
      await act(async () => { fireEvent.click(button); });
      expect(screen.getByTestId('delete-it-impact')).toHaveTextContent('Impact inconnu');
      // The deletion itself is not blocked by an unreadable count.
      expect(button).toHaveAttribute('data-armed', 'true');
      expect(button).not.toBeDisabled();
    });

    it('drops a result that belongs to an earlier arming', async () => {
      vi.useFakeTimers();
      try {
        let calls = 0;
        const button = renderWithImpact(() => {
          calls += 1;
          const text = `arming ${calls}`;
          return new Promise(resolve => { window.setTimeout(() => resolve(text), calls === 1 ? 7000 : 100); });
        });
        fireEvent.click(button);
        // Disarmed by the timer before the slow first count lands…
        await act(async () => { await vi.advanceTimersByTimeAsync(5001); });
        expect(button).toHaveAttribute('data-armed', 'false');
        // …re-armed, and only the second arming's count may describe it.
        fireEvent.click(button);
        await act(async () => { await vi.advanceTimersByTimeAsync(2500); });
        expect(screen.getByTestId('delete-it-impact')).toHaveTextContent('arming 2');
      } finally {
        vi.useRealTimers();
      }
    });
  });

});
