import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { Archive } from 'lucide-react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { CollectionRowActions } from '../CollectionRowActions';
import { CollectionSidebarFooter } from '../CollectionSidebarFooter';

describe('CollectionRowActions', () => {
  const writeText = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    writeText.mockResolvedValue(undefined);
  });

  it('shares the favorite, copy and contextual action contract', async () => {
    const onToggle = vi.fn();
    const onArchive = vi.fn();
    render(<CollectionRowActions
      itemName="Alpha"
      favorite={{ active: false, onToggle, activeLabel: 'Remove favorite', inactiveLabel: 'Add favorite' }}
      menuLabel="Actions"
      copyId="project-alpha"
      copyLabel="Copy ID"
      actions={[{ id: 'archive', label: 'Archive', icon: <Archive size={12} />, onSelect: onArchive }]}
    />);

    fireEvent.click(screen.getByRole('button', { name: 'Add favorite · Alpha' }));
    expect(onToggle).toHaveBeenCalledOnce();

    const menuButton = screen.getByRole('button', { name: 'Actions · Alpha' });
    fireEvent.click(menuButton);
    expect(menuButton).toHaveAttribute('aria-expanded', 'true');
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy ID' }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('project-alpha'));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Archive' }));
    expect(onArchive).toHaveBeenCalledOnce();
    expect(menuButton).toHaveAttribute('aria-expanded', 'false');
  });

  it('renders the shared navigation and search hints', () => {
    render(<CollectionSidebarFooter label="Projects" navigateLabel="navigate" searchLabel="search" />);
    expect(screen.getByText('↑↓')).toBeInTheDocument();
    expect(screen.getByText('/')).toBeInTheDocument();
    expect(screen.getByText('Projects')).toBeInTheDocument();
  });
});
