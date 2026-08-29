import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { CollectionFavoritesHeader } from '../CollectionFavoritesHeader';

describe('CollectionFavoritesHeader', () => {
  it('renders the canonical Favorites appearance and collapse interaction', () => {
    const onToggle = vi.fn();
    const { container } = render(
      <CollectionFavoritesHeader
        label="Favorites"
        count={3}
        expanded
        onToggle={onToggle}
      />,
    );

    const header = screen.getByRole('button', { name: 'Favorites 3' });
    expect(header).toHaveClass('collection-favorites-header');
    expect(header).toHaveAttribute('aria-expanded', 'true');
    expect(container.querySelector('.collection-favorites-header-star')).toBeInTheDocument();
    expect(container.querySelector('.collection-favorites-header-star')).toHaveAttribute('fill', 'none');

    fireEvent.click(header);
    expect(onToggle).toHaveBeenCalledOnce();
  });
});
