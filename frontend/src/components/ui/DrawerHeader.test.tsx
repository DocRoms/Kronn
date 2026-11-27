import { describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { DrawerHeader } from './DrawerHeader';
import { StatusBadge } from './StatusBadge';
import { describeHeaderContract } from '../../test/headerContract';
import { describeViolations, runAxe } from '../../test/axe';

describeHeaderContract('DrawerHeader', DrawerHeader, 'drawer');

describe('DrawerHeader', () => {
  it('names its layer (h2) and closes from one named button, the last control', async () => {
    const onClose = vi.fn();
    render(
      <main>
        <aside aria-labelledby="drawer-title">
          <DrawerHeader
            titleId="drawer-title"
            title="My discussion"
            status={<StatusBadge tone="info">Shared</StatusBadge>}
            actions={<button type="button">Refresh</button>}
            onClose={onClose}
            closeLabel="Close the panels"
            closeTestId="drawer-close"
          />
        </aside>
      </main>,
    );
    expect(screen.getByRole('complementary', { name: 'My discussion' })).toBeInTheDocument();
    const buttons = screen.getAllByRole('button');
    expect(buttons.map(b => b.getAttribute('aria-label') ?? b.textContent)).toEqual(['Refresh', 'Close the panels']);
    const close = screen.getByTestId('drawer-close');
    expect(close).toHaveAccessibleName('Close the panels');
    await userEvent.click(close);
    expect(onClose).toHaveBeenCalledTimes(1);
    const results = await act(() => runAxe(document.body));
    expect(describeViolations(results)).toEqual([]);
  });

  it('has no close button without onClose', () => {
    render(<DrawerHeader title="Settings" />);
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('refuses a close button without its name', () => {
    // @ts-expect-error — onClose without closeLabel does not compile.
    const bad = <DrawerHeader title="x" onClose={() => {}} />;
    expect(bad).toBeTruthy();
  });
});
