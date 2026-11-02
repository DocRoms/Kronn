import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { SidePanelShell } from '../SidePanelShell';

function Harness({ onDelete = vi.fn(), onClose = vi.fn() }: { onDelete?: () => void; onClose?: () => void }) {
  const [active, setActive] = useState('settings');
  return (
    <SidePanelShell
      testId="shell"
      ariaLabel="Panels"
      title="My discussion"
      railLabel="Discussion panels"
      activeId={active}
      onSelect={setActive}
      onClose={onClose}
      closeLabel="Close"
      menuLabel="More actions"
      menuActions={[
        { id: 'export', label: 'Export', icon: null, onSelect: vi.fn() },
        { id: 'delete', label: 'Delete', icon: null, danger: true, onSelect: onDelete },
      ]}
      items={[
        { id: 'settings', label: 'Settings', description: 'Discussion settings', icon: null },
        { id: 'plan', label: 'Plan', icon: null, badge: 3 },
        { id: 'git', label: 'Git', icon: null },
      ]}
    >
      <p>body of {active}</p>
    </SidePanelShell>
  );
}

describe('SidePanelShell', () => {
  it('renders a vertical tablist named by the visible labels, with the title in the header', () => {
    render(<Harness />);
    expect(screen.getByRole('heading', { name: 'My discussion' })).toBeInTheDocument();
    const rail = screen.getByRole('tablist', { name: 'Discussion panels' });
    expect(rail).toHaveAttribute('aria-orientation', 'vertical');
    const settings = screen.getByRole('tab', { name: 'Settings' });
    expect(settings).toHaveAttribute('aria-selected', 'true');
    expect(settings).toHaveAttribute('title', 'Discussion settings');
    expect(screen.getByRole('tabpanel')).toHaveTextContent('body of settings');
    expect(screen.getByRole('tab', { name: /Plan/ })).toHaveTextContent('3');
  });

  it('shows a badge only on the tab that has one, and carries no badge otherwise', () => {
    const { container } = render(<Harness />);
    const badges = container.querySelectorAll('.side-panel-shell-badge');
    expect(badges).toHaveLength(1);
    expect(screen.getByRole('tab', { name: /Plan/ }).querySelector('.side-panel-shell-badge')?.textContent).toBe('3');
    expect(screen.getByRole('tab', { name: 'Git' }).querySelector('.side-panel-shell-badge')).toBeNull();
  });

  it('moves between tabs with the arrow keys, Home and End', () => {
    render(<Harness />);
    const rail = screen.getByRole('tablist');
    fireEvent.keyDown(rail, { key: 'ArrowDown' });
    expect(screen.getByRole('tab', { name: /Plan/ })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tab', { name: /Plan/ })).toHaveFocus();
    fireEvent.keyDown(rail, { key: 'End' });
    expect(screen.getByRole('tab', { name: 'Git' })).toHaveAttribute('aria-selected', 'true');
    fireEvent.keyDown(rail, { key: 'ArrowDown' });
    expect(screen.getByRole('tab', { name: 'Settings' })).toHaveAttribute('aria-selected', 'true');
    fireEvent.keyDown(rail, { key: 'ArrowUp' });
    expect(screen.getByRole('tab', { name: 'Git' })).toHaveAttribute('aria-selected', 'true');
  });

  it('runs actions from the « ⋯ » menu and closes from the header', () => {
    const onDelete = vi.fn();
    const onClose = vi.fn();
    render(<Harness onDelete={onDelete} onClose={onClose} />);
    expect(screen.queryByRole('menu')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete' }));
    expect(onDelete).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('menu')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('closes the menu on Escape without letting the key reach the page', () => {
    const pageEscape = vi.fn();
    window.addEventListener('keydown', pageEscape);
    render(<Harness />);
    fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
    fireEvent.keyDown(screen.getByRole('menuitem', { name: 'Export' }), { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(pageEscape).not.toHaveBeenCalled();
    window.removeEventListener('keydown', pageEscape);
  });
});
