/**
 * KT-464 — the one favorite/pin toggle shared by Discussions, Automations
 * and Pages row favorites. Pins the contract every caller relies on instead
 * of re-implementing: aria-pressed, a name contextualized by the item,
 * neutral→accent state via data-active, and that a click never also
 * triggers whatever bigger clickable row it sits inside.
 */
import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { FavoriteToggle } from '../FavoriteToggle';

describe('FavoriteToggle', () => {
  it('renders the inactive state with the inactive label and no accessible-name suffix when itemName is omitted', () => {
    render(
      <FavoriteToggle active={false} onToggle={vi.fn()} activeLabel="Remove from favorites" inactiveLabel="Add to favorites" />,
    );
    const button = screen.getByRole('button', { name: 'Add to favorites' });
    expect(button).toHaveAttribute('aria-pressed', 'false');
    expect(button).toHaveAttribute('data-active', 'false');
    expect(button).toHaveAttribute('title', 'Add to favorites');
  });

  it('renders the active state with the active label and data-active for the neutral→accent color switch', () => {
    render(
      <FavoriteToggle active onToggle={vi.fn()} activeLabel="Remove from favorites" inactiveLabel="Add to favorites" />,
    );
    const button = screen.getByRole('button', { name: 'Remove from favorites' });
    expect(button).toHaveAttribute('aria-pressed', 'true');
    expect(button).toHaveAttribute('data-active', 'true');
  });

  it('contextualizes the accessible name with the item name so identical rows are distinguishable', () => {
    render(
      <FavoriteToggle
        active={false}
        onToggle={vi.fn()}
        activeLabel="Remove from favorites"
        inactiveLabel="Add to favorites"
        itemName="Adobe Signals"
      />,
    );
    expect(screen.getByRole('button', { name: 'Add to favorites · Adobe Signals' })).toBeInTheDocument();
  });

  it('calls onToggle on click and never lets the click bubble into a wrapping clickable row', () => {
    const onToggle = vi.fn();
    const onRowClick = vi.fn();
    render(
      <div onClick={onRowClick}>
        <FavoriteToggle active={false} onToggle={onToggle} activeLabel="Remove" inactiveLabel="Add" />
      </div>,
    );
    fireEvent.click(screen.getByRole('button'));
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(onRowClick).not.toHaveBeenCalled();
  });

  it('accepts a layout-only className without losing the shared kr-favorite-toggle identity', () => {
    render(
      <FavoriteToggle active={false} onToggle={vi.fn()} activeLabel="Remove" inactiveLabel="Add" className="automation-resource-pin" />,
    );
    const button = screen.getByRole('button');
    expect(button.className.split(' ')).toEqual(expect.arrayContaining(['kr-favorite-toggle', 'automation-resource-pin']));
  });
});
