import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { CopyIdPill } from '../CopyIdPill';

describe('CopyIdPill', () => {
  it('copies the full ID while displaying its compact form and confirms it visually', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });

    render(<CopyIdPill id="12345678-abcd-4def-9012-123456789abc" title="Copy ID" />);

    const button = screen.getByRole('button', { name: 'Copy ID' });
    expect(button).toHaveTextContent('#12345678');

    fireEvent.click(button);

    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith('12345678-abcd-4def-9012-123456789abc');
      expect(button).toHaveAttribute('data-copied', 'true');
    });
  });
});
