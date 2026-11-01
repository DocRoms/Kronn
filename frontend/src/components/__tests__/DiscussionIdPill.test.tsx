/**
 * KT-1530 — the discussion id is back next to the title (it had moved into the
 * « Détails » side panel). These assertions moved here from the details
 * section's spec; what they guard did not change, only where it is rendered.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { DiscussionIdPill } from '../DiscussionIdPill';

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key);
const ID = '12345678-90ab-cdef-1234-567890abcdef';

describe('DiscussionIdPill', () => {
  it('shows the short id, and the tour anchor sits on it', () => {
    const { container } = render(<DiscussionIdPill discussionId={ID} toast={vi.fn()} t={t} />);
    const pill = container.querySelector<HTMLButtonElement>('.disc-id-pill');
    expect(pill).toHaveTextContent('#12345678');
    expect(pill).toHaveAttribute('data-tour-id', 'discussion-id-pill');
  });

  it('copies the full id and shows copied feedback', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    const toast = vi.fn();
    const { container } = render(<DiscussionIdPill discussionId={ID} toast={toast} t={t} />);
    const pill = container.querySelector<HTMLButtonElement>('.disc-id-pill')!;

    fireEvent.click(pill);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(ID));
    await waitFor(() => expect(pill.getAttribute('data-copied')).toBe('true'));
    expect(pill.querySelector('svg')).not.toBeNull();
    expect(toast).toHaveBeenCalledWith('disc.idCopied', 'success');
  });

  it('says so when the clipboard refuses', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockRejectedValue(new Error('denied')) },
      configurable: true,
    });
    const toast = vi.fn();
    const { container } = render(<DiscussionIdPill discussionId={ID} toast={toast} t={t} />);

    fireEvent.click(container.querySelector('.disc-id-pill')!);
    await waitFor(() => expect(toast).toHaveBeenCalledWith('disc.idCopyFailed', 'error'));
  });
});
