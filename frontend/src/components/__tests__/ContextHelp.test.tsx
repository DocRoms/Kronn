import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ContextHelp } from '../ContextHelp';

describe('ContextHelp', () => {
  it('opens on demand and closes with Escape', async () => {
    render(
      <ContextHelp title="Understand this">
        <p>Contextual explanation</p>
      </ContextHelp>,
    );

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Understand this' }));
    expect(screen.getByRole('dialog')).toHaveTextContent('Contextual explanation');

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Understand this' })).toHaveFocus());
  });

  it('closes when the user clicks outside', () => {
    render(
      <div>
        <ContextHelp title="Help">
          <p>Details</p>
        </ContextHelp>
        <button type="button">Outside</button>
      </div>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Help' }));
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Outside' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
