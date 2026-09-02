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
});
